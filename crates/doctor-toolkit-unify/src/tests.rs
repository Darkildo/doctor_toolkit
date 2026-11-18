//! Two fake editions, the unified game made from them, a pack, and the
//! unified game made back from every kind of source.

use std::fs;
use std::path::{Path, PathBuf};

use flate2::{Compress, Compression, FlushCompress};

use super::*;
use crate::big::build_archive;

fn write(path: &Path, data: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

/// Bytes that compress but do not repeat within 32 768 bytes.
fn noise(seed: u32, len: usize) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|i| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if i % 7 < 4 {
                (state >> 16) as u8
            } else {
                b'a' + (i % 5) as u8
            }
        })
        .collect()
}

/// A cabinet with one MSZIP folder (or a stored one) holding `files`.
fn cabinet(files: &[(&str, &[u8])], mszip: bool) -> Vec<u8> {
    let data: Vec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let mut blocks = Vec::new();
    for (i, chunk) in data.chunks(32_768).enumerate() {
        let stored = if mszip {
            let mut deflate = Compress::new(Compression::default(), false);
            if i > 0 {
                deflate
                    .set_dictionary(&data[(i - 1) * 32_768..i * 32_768])
                    .unwrap();
            }
            let mut out = Vec::with_capacity(chunk.len() + 1024);
            deflate
                .compress_vec(chunk, &mut out, FlushCompress::Finish)
                .unwrap();
            [b"CK".as_slice(), &out].concat()
        } else {
            chunk.to_vec()
        };
        blocks.push((stored, chunk.len()));
    }
    let mut entries = Vec::new();
    let mut offset = 0u32;
    for (name, d) in files {
        entries.extend_from_slice(&(d.len() as u32).to_le_bytes());
        entries.extend_from_slice(&offset.to_le_bytes());
        entries.extend_from_slice(&[0; 8]); // folder 0, date, time, attributes
        entries.extend_from_slice(name.as_bytes());
        entries.push(0);
        offset += d.len() as u32;
    }
    let files_at = 36 + 8;
    let data_at = files_at + entries.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"MSCF\0\0\0\0\0\0\0\0\0\0\0\0");
    out.extend_from_slice(&(files_at as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 3, 1]);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 6]); // flags, set id, cabinet number
    out.extend_from_slice(&(data_at as u32).to_le_bytes());
    out.extend_from_slice(&(blocks.len() as u16).to_le_bytes());
    out.extend_from_slice(&(mszip as u16).to_le_bytes());
    out.extend_from_slice(&entries);
    for (stored, size) in blocks {
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(stored.len() as u16).to_le_bytes());
        out.extend_from_slice(&(size as u16).to_le_bytes());
        out.extend_from_slice(&stored);
    }
    out
}

fn table(tag: &str) -> String {
    format!("7\n454107DA\n{tag}\nNOT SET\n")
}

fn sounds(names: &[(&str, &[u8])]) -> Vec<u8> {
    build_archive(
        &names
            .iter()
            .map(|(n, d)| (n.to_string(), d.to_vec()))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

/// `ntsc`, `pal` (installed copies), `ntsc-disc`, `pal-disc` (cabinets),
/// `unified` and the pack `pack`.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let shared = noise(1, 70_000);
    let world_ntsc = noise(2, 40_000);
    let world_pal = noise(3, 40_000);
    let (english, spanish, russian) = (noise(4, 5_000), noise(5, 3_000), noise(6, 4_000));

    for (id, tag, world, second, speech) in [
        ("ntsc", "ntsc_en", &world_ntsc, "es", &spanish),
        ("pal", "pal_en", &world_pal, "ru", &russian),
    ] {
        let tree = root.join(id);
        write(&tree.join("text/localetable.txt"), table(tag));
        write(&tree.join("gf2/shared.str"), &shared);
        write(&tree.join("gf2/world.str"), world);
        write(&tree.join("movies/en/load.vp6"), b"english movie");
        write(
            &tree.join(format!("movies/{second}/load.vp6")),
            format!("{second} movie"),
        );
        write(&tree.join("godfather2.exe"), format!("protected {id}"));
        let second_name = format!("audiostreams\\{second}\\line.exa.snu");
        write(
            &tree.join("GODFATH0.viv"),
            sounds(&[
                ("audiostreams\\line.exa.snu", &english),
                (&second_name, speech),
            ]),
        );
        // The disc: streams and the table in cabinets, the rest as files.
        let disc = root.join(format!("{id}-disc"));
        write(
            &disc.join("data_gf.cab"),
            cabinet(&[("gf2Shared_str", &shared), ("gf2World_str", world)], true),
        );
        write(
            &disc.join("data_t14.cab"),
            cabinet(&[("textLocaletable_txt", table(tag).as_bytes())], false),
        );
        for plain in ["movies/en/load.vp6", "GODFATH0.viv"] {
            write(&disc.join(plain), fs::read(tree.join(plain)).unwrap());
        }
        write(
            &disc.join(format!("movies/{second}/load.vp6")),
            format!("{second} movie"),
        );
        write(
            &root.join(format!("{id}.tsv")),
            "gf2/shared.str\tdata_gf.cab\tgf2Shared_str\t70000\n\
             gf2/world.str\tdata_gf.cab\tgf2World_str\t40000\n\
             text/localetable.txt\tdata_t14.cab\ttextLocaletable_txt\t30\n\
             movies/en/load.vp6\t-\tmovies/en/load.vp6\t13\n",
        );
    }

    // The unified game: the NTSC data, a new executable and table, both
    // second languages, Russian speech in its own archive.
    let unified = root.join("unified");
    write(
        &unified.join("text/localetable.txt"),
        table("ntsc_en") + "ru\n",
    );
    write(&unified.join("gf2/shared.str"), &shared);
    write(&unified.join("gf2/world.str"), &world_ntsc);
    write(&unified.join("movies/en/load.vp6"), b"english movie");
    write(&unified.join("movies/es/load.vp6"), b"es movie");
    write(&unified.join("movies/ru/load.vp6"), b"ru movie");
    write(&unified.join("godfather2.exe"), b"unprotected");
    fs::copy(root.join("ntsc/GODFATH0.viv"), unified.join("GODFATH0.viv")).unwrap();
    write(
        &unified.join("GODFATH3.viv"),
        sounds(&[("audiostreams\\ru\\line.exa.snu", &russian)]),
    );

    let editions: Vec<EditionTree> = [("ntsc", "for-ntsc"), ("pal", "for-pal")]
        .iter()
        .map(|(id, part)| EditionTree {
            id: id.to_string(),
            name: format!("the {id} disc"),
            part: part.to_string(),
            tree: root.join(id),
            disc_map: Some(root.join(format!("{id}.tsv"))),
        })
        .collect();
    build_pack("test", &unified, &editions, &root.join("pack"), |_| {}).unwrap();
    Fixture { _dir: dir, root }
}

fn same_trees(a: &Path, b: &Path) {
    let files = tree_files(a).unwrap();
    assert_eq!(files, tree_files(b).unwrap());
    for file in files {
        assert_eq!(
            fs::read(a.join(&file)).unwrap(),
            fs::read(b.join(&file)).unwrap(),
            "{file}"
        );
    }
}

#[test]
fn cabinet_files_come_out_whole_across_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (noise(7, 50_000), noise(8, 0), noise(9, 90_001));
    for mszip in [true, false] {
        let path = dir.path().join("x.cab");
        fs::write(
            &path,
            cabinet(&[("a", &a), ("empty", &b), ("c", &c)], mszip),
        )
        .unwrap();
        assert_eq!(cab::read_file(&path, "A").unwrap(), a);
        assert_eq!(cab::read_file(&path, "empty").unwrap(), b);
        assert_eq!(cab::read_file(&path, "c").unwrap(), c);
        assert!(cab::read_file(&path, "d").is_err());
    }
}

#[test]
fn the_pack_holds_only_what_an_edition_lacks() {
    let f = fixture();
    let pack = f.path("pack");
    let manifest = Manifest::load(&pack).unwrap();
    let part = |path: &str| {
        manifest
            .files
            .iter()
            .find(|x| x.path == path)
            .unwrap()
            .part
            .clone()
    };
    assert_eq!(part("gf2/shared.str"), None);
    // Small files travel in the pack whoever has them.
    assert_eq!(part("movies/en/load.vp6").as_deref(), Some(PART_COMMON));
    assert_eq!(part("gf2/world.str").as_deref(), Some("for-pal"));
    assert_eq!(part("movies/ru/load.vp6").as_deref(), Some("for-ntsc"));
    assert_eq!(part("godfather2.exe").as_deref(), Some(PART_COMMON));
    assert!(pack.join("for-pal/gf2/world.str").is_file());
    assert!(!pack.join("for-pal/gf2/shared.str").exists());
    // Spanish speech for the PAL owner, Russian for the NTSC owner.
    let lacking = |path: &str| {
        manifest
            .archives
            .iter()
            .find(|a| a.path == path)
            .unwrap()
            .lacking
            .clone()
    };
    assert_eq!(lacking("GODFATH0.viv").keys().collect::<Vec<_>>(), ["pal"]);
    assert_eq!(lacking("GODFATH3.viv").keys().collect::<Vec<_>>(), ["ntsc"]);
    let spanish = BigArchive::open(pack.join("for-pal/GODFATH0.viv")).unwrap();
    assert_eq!(spanish.entries.len(), 1);
    assert_eq!(spanish.entries[0].name, "audiostreams\\es\\line.exa.snu");
}

#[test]
fn every_source_gives_the_same_game() {
    let f = fixture();
    for source in ["ntsc", "pal", "ntsc-disc", "pal-disc"] {
        let out = f.path(&format!("out-{source}"));
        let report = apply(&f.path("pack"), &f.path(source), &out, &mut |_, _, _| {}).unwrap();
        same_trees(&f.path("unified"), &out);
        assert!(report.changed_in_source.is_empty(), "{source}");
        assert_eq!(report.archives_built, 2);
        assert!(verify(&f.path("pack"), &out).unwrap().is_empty());
        // A second run has nothing to do.
        let again = apply(&f.path("pack"), &f.path(source), &out, &mut |_, _, _| {}).unwrap();
        assert_eq!(
            (again.from_source, again.from_pack, again.archives_built),
            (0, 0, 0)
        );
    }
}

#[test]
fn a_changed_copy_is_repaired_from_a_full_pack_or_refused() {
    let f = fixture();
    // A mod in the installed NTSC copy; the pack has that stream for PAL owners.
    write(&f.path("ntsc/gf2/world.str"), b"modded");
    let out = f.path("out");
    let report = apply(&f.path("pack"), &f.path("ntsc"), &out, &mut |_, _, _| {}).unwrap();
    assert_eq!(report.changed_in_source, ["gf2/world.str"]);
    same_trees(&f.path("unified"), &out);

    // Nothing has the shared stream but the copies themselves.
    write(&f.path("ntsc/gf2/shared.str"), b"modded");
    let error = apply(
        &f.path("pack"),
        &f.path("ntsc"),
        &f.path("out2"),
        &mut |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("gf2/shared.str"), "{error}");
    assert!(!f.path("out2/gf2/shared.str").exists());

    // Changed speech: the archive would differ from the unified one.
    let english = noise(10, 5_000);
    write(
        &f.path("pal/GODFATH0.viv"),
        sounds(&[
            ("audiostreams\\line.exa.snu", &english),
            ("audiostreams\\ru\\line.exa.snu", &noise(6, 4_000)),
        ]),
    );
    let error = apply(
        &f.path("pack"),
        &f.path("pal"),
        &f.path("out3"),
        &mut |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("GODFATH0.viv"), "{error}");
    assert!(!f.path("out3/GODFATH0.viv").exists());
}

#[test]
fn a_copy_with_unpacked_sounds_and_no_small_files_still_works() {
    let f = fixture();
    let pal = f.path("pal");
    let archive = BigArchive::open(pal.join("GODFATH0.viv")).unwrap();
    for entry in &archive.entries {
        write(
            &pal.join(entry.name.replace('\\', "/")),
            archive.read(entry).unwrap(),
        );
    }
    fs::remove_file(pal.join("GODFATH0.viv")).unwrap();
    fs::remove_file(pal.join("movies/en/load.vp6")).unwrap();
    let out = f.path("out");
    let report = apply(&f.path("pack"), &pal, &out, &mut |_, _, _| {}).unwrap();
    assert_eq!(report.changed_in_source, ["movies/en/load.vp6"]);
    same_trees(&f.path("unified"), &out);
}

#[test]
fn the_source_must_be_a_known_edition_and_another_folder() {
    let f = fixture();
    let error = apply(
        &f.path("pack"),
        &f.path("pack"),
        &f.path("o"),
        &mut |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("neither a disc"), "{error}");
    write(&f.path("other/text/localetable.txt"), table("jp_en"));
    let error = apply(
        &f.path("pack"),
        &f.path("other"),
        &f.path("o"),
        &mut |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("jp_en"), "{error}");
    let error = apply(
        &f.path("pack"),
        &f.path("ntsc"),
        &f.path("ntsc"),
        &mut |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("another one"), "{error}");
}

#[test]
fn the_locale_line_of_the_settings_is_replaced_in_place() {
    let text = "Audio.Music = 1.0 \r\nLocale = en \r\nMouse.Invert = false \r\n";
    assert_eq!(
        locale::with_locale(text, "ru").unwrap(),
        "Audio.Music = 1.0 \r\nLocale = ru \r\nMouse.Invert = false \r\n"
    );
    assert_eq!(locale::with_locale("Audio.Music = 1.0 \n", "ru"), None);
    assert!(locale::set_language("xx").is_err());
    assert!(locale::registry_command("pl").contains("/d pl "));
}

#[test]
fn a_pack_without_the_part_of_the_edition_is_told_before_the_run() {
    let f = fixture();
    let pack = f.path("pack");
    let manifest = Manifest::load(&pack).unwrap();
    let (ntsc, pal) = (
        manifest.edition("ntsc").unwrap(),
        manifest.edition("pal").unwrap(),
    );
    assert!(pack_gaps(&manifest, &pack, ntsc).is_empty());
    assert!(pack_gaps(&manifest, &pack, pal).is_empty());

    // The owner of the NTSC disc left the PAL part out of the download.
    fs::remove_dir_all(pack.join("for-pal")).unwrap();
    assert!(pack_gaps(&manifest, &pack, ntsc).is_empty());
    let gaps = pack_gaps(&manifest, &pack, pal);
    assert!(
        gaps.contains(&"for-pal/gf2/world.str".to_string()),
        "{gaps:?}"
    );
    assert!(
        gaps.contains(&"for-pal/GODFATH0.viv".to_string()),
        "{gaps:?}"
    );
    let out = f.path("out");
    apply(&pack, &f.path("ntsc-disc"), &out, &mut |_, _, _| {}).unwrap();
    same_trees(&f.path("unified"), &out);

    // A file the download has not finished: the size gives it away.
    write(&pack.join("for-ntsc/movies/ru/load.vp6"), b"half");
    assert_eq!(
        pack_gaps(&manifest, &pack, ntsc),
        ["for-ntsc/movies/ru/load.vp6"]
    );
}

#[test]
fn only_a_published_pack_is_known() {
    let f = fixture();
    assert_eq!(known_pack(&f.path("pack")).unwrap(), None);
    let latest = latest_pack();
    assert_eq!(latest.manifest_sha256.len(), 64);
    assert!(latest.magnet.starts_with("magnet:?xt=urn:btih:"));
    assert!(!latest.magnet.contains(' '));
}
