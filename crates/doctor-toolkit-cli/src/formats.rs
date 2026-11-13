//! Export commands for rigs, fonts, audio, scripts and the smaller resource types.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use doctor_toolkit::game::{GameFile, GameInstance};

/// Writes a line to stdout and exits quietly when the pipe is closed (`| head`).
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// Calls `f` with the data of every resource of `type_id` in the game.
///
/// `f` receives the stream, the resource index, its last id and its data.
fn each_resource(
    game: &GameInstance,
    type_id: u32,
    mut f: impl FnMut(&GameFile, usize, u32, &[u8]) -> Result<()>,
) -> Result<usize> {
    let mut count = 0;
    for (stream_index, file) in game.streams().iter().enumerate() {
        let stream = game.open_stream(stream_index)?;
        let layout = stream.layout();
        for (index, resource) in layout.resources.iter().enumerate() {
            if resource.header.type_id != type_id {
                continue;
            }
            let data = stream
                .resource_data(index)
                .with_context(|| format!("{} resource {index}", file.rel_path))?;
            f(file, index, resource.header.ids[3], &data)
                .with_context(|| format!("{} resource {index}", file.rel_path))?;
            count += 1;
        }
    }
    Ok(count)
}

/// `<output>/<stream path>/`, created.
fn stream_dir(output: &Path, file: &GameFile) -> Result<PathBuf> {
    let dir = output.join(file.rel_path.trim_end_matches(".str"));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(value)? + "\n")?;
    Ok(())
}

/// `skeletons`: every rig (`rcb`) as JSON with bind-pose transforms.
pub fn skeletons(game_dir: &Path, output: &Path) -> Result<()> {
    use doctor_toolkit::resource::rcb::{Rig, TYPE_RCB, translation};

    let game = GameInstance::open(game_dir)?;
    let mut bones = 0;
    let count = each_resource(&game, TYPE_RCB, |file, index, id, data| {
        let rig = Rig::parse(data)?;
        let skeletons: Vec<Value> = rig
            .skeletons
            .iter()
            .map(|skeleton| {
                let globals = skeleton.bind_globals();
                let locals = skeleton.bind_locals();
                let list: Vec<Value> = skeleton
                    .bones
                    .iter()
                    .enumerate()
                    .map(|(i, bone)| {
                        json!({
                            "index": i,
                            "name": bone.name,
                            "hash": format!("{:08x}", bone.hash),
                            "parent": bone.parent,
                            "link": bone.bind_index,
                            "kind": bone.bind_type,
                            "position": translation(&globals[i]),
                            "local_matrix": locals[i],
                            "inverse_bind_matrix": bone.inverse_bind,
                        })
                    })
                    .collect();
                bones += list.len();
                json!({
                    "name": skeleton.name,
                    "name_hash": format!("{:08x}", skeleton.name_hash),
                    "bones": list,
                })
            })
            .collect();
        let sets: Vec<Value> = rig
            .bone_sets
            .iter()
            .map(|s| json!({ "hash": format!("{:08x}", s.hash), "bones": s.bones }))
            .collect();
        let document = json!({
            "stream": file.rel_path,
            "resource": index,
            "id": format!("{id:08x}"),
            "version": rig.version,
            "hash": format!("{:08x}", rig.hash),
            "scale": rig.scale,
            "matrix_convention": "column-major, column vectors (glTF)",
            "skeletons": skeletons,
            "bone_sets": sets,
        });
        let path = stream_dir(output, file)?.join(format!("{index:05}_rcb_{id:08x}.json"));
        write_json(&path, &document)
    })?;
    out!("exported {count} rig(s), {bones} bone(s)");
    Ok(())
}

/// `audio`: sounds of `sbk` banks and the streamed `.snu` files of the `.viv` archives.
pub fn audio(
    game_dir: &Path,
    output: &Path,
    wav: bool,
    name_filter: Option<&str>,
    banks_only: bool,
) -> Result<()> {
    use doctor_toolkit::container::big::{BigArchive, find_archives};
    use doctor_toolkit::resource::audio::{Sample, TYPE_SBK, parse_sbk, to_snu};

    let game = GameInstance::open(game_dir)?;
    let filter = name_filter.map(str::to_ascii_lowercase);
    let wanted = |name: &str| {
        filter
            .as_ref()
            .is_none_or(|f| name.to_ascii_lowercase().contains(f))
    };
    let mut written = 0usize;
    let mut seconds = 0f64;
    each_resource(&game, TYPE_SBK, |file, index, id, data| {
        let bank = format!("{index:05}_sbk_{id:08x}");
        if !wanted(&format!("{}/{bank}", file.rel_path)) {
            return Ok(());
        }
        let dir = stream_dir(output, file)?.join(&bank);
        fs::create_dir_all(&dir)?;
        for sound in parse_sbk(data)? {
            let sample = &sound.sample;
            let path = dir.join(format!("{:04}", sound.index));
            if wav {
                fs::write(path.with_extension("wav"), sample.to_wav()?)?;
            } else {
                fs::write(path.with_extension("snu"), to_snu(data, sample))?;
            }
            seconds += sample.duration();
            written += 1;
        }
        Ok(())
    })?;
    if !banks_only {
        let mut seen = std::collections::HashSet::new();
        for archive_path in find_archives(game_dir)? {
            let archive = BigArchive::open(&archive_path)?;
            for entry in &archive.entries {
                // The game reads a name from the first archive that has it;
                // the overlay's marker is the toolkit's.
                if !wanted(&entry.name)
                    || doctor_toolkit::container::big::is_tool_entry(&entry.name)
                    || !seen.insert(doctor_toolkit::container::big::normalize_name(&entry.name))
                {
                    continue;
                }
                let data = archive.read(entry)?;
                let sample = Sample::parse_snu(&data).with_context(|| entry.name.clone())?;
                seconds += sample.duration();
                let name = if wav {
                    format!("{}.wav", entry.name.trim_end_matches(".exa.snu"))
                } else {
                    entry.name.clone()
                };
                let path = output.join(name.split('\\').collect::<PathBuf>());
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                if wav {
                    fs::write(&path, sample.to_wav()?)?;
                } else {
                    fs::write(&path, &data)?;
                }
                written += 1;
            }
        }
    }
    out!(
        "exported {written} sound(s), {:.1} minute(s)",
        seconds / 60.0
    );
    Ok(())
}

/// Keeps letters, digits, `-`, `_` and `.` of a file name.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_owned()
    } else {
        cleaned
    }
}

/// `fonts`: every font (`ffn`) as a PNG atlas plus JSON glyph metrics.
pub fn fonts(game_dir: &Path, output: &Path) -> Result<()> {
    use doctor_toolkit::resource::ffn::{Font, TYPE_FFN};

    let game = GameInstance::open(game_dir)?;
    let mut atlases = 0;
    let mut count = 0;
    for (stream_index, file) in game.streams().iter().enumerate() {
        let stream = game.open_stream(stream_index)?;
        let layout = stream.layout();
        for (index, resource) in layout.resources.iter().enumerate() {
            if resource.header.type_id != TYPE_FFN {
                continue;
            }
            let font = Font::parse(&stream.resource_data(index)?)
                .with_context(|| format!("{} resource {index}", file.rel_path))?;
            let dir = stream_dir(output, file)?;
            let base = dir.join(sanitize(&font.name));
            let atlas = font.find_atlas(stream.data(), layout);
            if let Some(texture) = &atlas {
                fs::write(base.with_extension("png"), texture.to_png(0)?)?;
                atlases += 1;
            }
            let glyphs: Vec<Value> = font
                .glyphs
                .iter()
                .map(|g| {
                    json!({
                        "char": g.character().map(String::from),
                        "code": format!("U+{:04X}", g.code),
                        "width": g.width,
                        "height": g.height,
                        "advance": g.advance,
                        "x_offset": g.x_offset,
                        "y_offset": g.y_offset,
                        "atlas_rect": g.rect,
                    })
                })
                .collect();
            let document = json!({
                "stream": file.rel_path,
                "resource": index,
                "name": font.name,
                "version": font.version,
                "metrics": font.metrics,
                "atlas": atlas.as_ref().map(|t| json!({
                    "file": format!("{}.png", sanitize(&font.name)),
                    "width": t.header.width,
                    "height": t.header.height,
                })),
                "pages": font.pages.iter().map(|p| format!("U+{p:02X}xx")).collect::<Vec<_>>(),
                "kerning_entries": font.kerning_count,
                "glyphs": glyphs,
            });
            write_json(&base.with_extension("json"), &document)?;
            out!(
                "{}: {} glyph(s){}",
                base.display(),
                font.glyphs.len(),
                if atlas.is_some() {
                    ""
                } else {
                    ", atlas not found"
                }
            );
            count += 1;
        }
    }
    out!("exported {count} font(s), {atlases} atlas(es)");
    Ok(())
}

/// `lua-export`: every script (`lua`) as a Lua 5.0 `.luac` file named after its source.
pub fn lua_export(game_dir: &Path, output: &Path) -> Result<()> {
    use doctor_toolkit::resource::lua::{LuaChunk, TYPE_LUA};

    let game = GameInstance::open(game_dir)?;
    let count = each_resource(&game, TYPE_LUA, |file, index, id, data| {
        let chunk = LuaChunk::parse(data)?;
        let stem = chunk.file_name().trim_end_matches(".lua");
        let name = if stem.is_empty() {
            format!("{index:05}_lua_{id:08x}")
        } else {
            sanitize(stem)
        };
        let path = stream_dir(output, file)?.join(format!("{name}.luac"));
        fs::write(&path, data)?;
        out!(
            "{} ({}, {} function(s))",
            path.display(),
            chunk.source,
            chunk.functions
        );
        Ok(())
    })?;
    out!("exported {count} script(s); Lua 5.0 bytecode with 4-byte float numbers");
    Ok(())
}
