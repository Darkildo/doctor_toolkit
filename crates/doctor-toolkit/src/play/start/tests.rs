//! `cmd.txt` contents and its write/restore cycle on a temporary folder.

use std::fs;

use super::*;

fn streams(list: &[&str]) -> StartOptions {
    StartOptions {
        streams: list.iter().map(|s| s.to_string()).collect(),
        ..StartOptions::default()
    }
}

#[test]
fn stream_comes_last_with_folder_and_names() {
    let options = StartOptions {
        streams: vec!["gf2/cuba".into()],
        spawn: Some("26d7c8323dfa11aa464f574c45523130".into()),
        windowed: Some(true),
        resolution: Some((1024, 768)),
    };
    assert_eq!(
        options.args().unwrap(),
        [
            "-windowed",
            "-resolution",
            "1024",
            "768",
            "-playerspawn",
            "{32C8D726-FA3D-AA11-464F-574C45523130}",
            "-stream",
            "gf2",
            "cuba",
        ]
    );
    assert_eq!(
        options.cmd_txt().unwrap(),
        "-windowed -resolution 1024 768 -playerspawn {32C8D726-FA3D-AA11-464F-574C45523130} -stream gf2 cuba\r\n"
    );
    let full = StartOptions {
        windowed: Some(false),
        ..StartOptions::default()
    };
    assert_eq!(full.args().unwrap(), ["-fullscreen"]);
    assert!(StartOptions::default().is_empty());
    assert_eq!(
        StartOptions::default().args().unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn streams_share_the_longest_folder() {
    let split = |list: &[&str]| split_streams(&streams(list).streams);
    assert_eq!(
        split(&["mp_mall/mp_mall"]).unwrap(),
        ("mp_mall".into(), vec!["mp_mall".into()])
    );
    // `.str` makes the game crash: it is dropped; backslashes are fine.
    assert_eq!(
        split(&["gf2\\cuba.str", "gf2/cuba/local.STR"]).unwrap(),
        ("gf2".into(), vec!["cuba".into(), "cuba\\local".into()])
    );
    assert_eq!(
        split(&["gf2/cuba/local", "gf2/cuba/global"]).unwrap(),
        ("gf2\\cuba".into(), vec!["local".into(), "global".into()])
    );
    assert_eq!(
        split(&["GF2/miami", "gf2/queens"]).unwrap(),
        ("GF2".into(), vec!["miami".into(), "queens".into()])
    );
    for bad in [
        &["cuba"][..],
        &["gf2/cuba", "mp_mall/mp_mall"],
        &["gf2/my level"],
        &["gf2/a*b"],
        &["#gf2/cuba"],
        &["gf2/../cuba"],
        &["gf2//cuba"],
        &[][..],
    ] {
        assert!(split(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn guids_in_the_games_text_form() {
    // An object GUID as the toolkit prints it (file order) and as the
    // game's parser 0x004D9500 reads it.
    assert_eq!(
        spawn_arg("26d7c8323dfa11aa464f574c45523130").unwrap(),
        "{32C8D726-FA3D-AA11-464F-574C45523130}"
    );
    // The game's own "no GUID": dwords BADBADBA BEEFBEEF EAC15A55 91100911.
    let mut bytes = Vec::new();
    for word in [0xBADB_ADBAu32, 0xBEEF_BEEF, 0xEAC1_5A55, 0x9110_0911] {
        bytes.extend(word.to_le_bytes());
    }
    let nil = Guid(bytes.try_into().unwrap());
    assert_eq!(braced(&nil), "{BADBADBA-BEEF-BEEF-555A-C1EA11091091}");
    assert_eq!(
        parse_braced("{badbadba-beef-beef-555a-c1ea11091091}").unwrap(),
        nil
    );
    assert_eq!(
        spawn_arg("{32c8d726-fa3d-aa11-464f-574c45523130}").unwrap(),
        "{32C8D726-FA3D-AA11-464F-574C45523130}"
    );
    for bad in [
        "26d7c832",
        "26d7c8323dfa11aa464f574c4552313g",
        "{32C8D726-FA3D-AA11-464F574C45523130}",
        "{32C8D726-FA3D-AA11-464F-574C45523130",
    ] {
        assert!(spawn_arg(bad).is_err(), "{bad}");
    }
    let lonely = StartOptions {
        spawn: Some("26d7c8323dfa11aa464f574c45523130".into()),
        ..StartOptions::default()
    };
    assert!(lonely.args().is_err(), "a spawn point without a stream");
    let zero = StartOptions {
        resolution: Some((0, 768)),
        ..StartOptions::default()
    };
    assert!(zero.args().is_err());
}

#[test]
fn cmd_file_is_taken_away_after_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let target = root.join(CMD_FILE);
    let options = streams(&["mp_mall/mp_mall"]);

    // No cmd.txt before: written, then removed.
    let file = CmdFile::write(root, &options).unwrap();
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "-stream mp_mall mp_mall\r\n"
    );
    assert!(file.restore().unwrap());
    assert!(!target.exists());
    assert!(!restore_stale(root).unwrap(), "nothing left to do");

    // The player's own cmd.txt comes back.
    fs::write(&target, "-windowed\r\n").unwrap();
    let file = CmdFile::write(root, &options).unwrap();
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "-stream mp_mall mp_mall\r\n"
    );
    drop(file);
    assert_eq!(fs::read_to_string(&target).unwrap(), "-windowed\r\n");

    // Stopped while the game ran: the next start (or restore_stale) cleans up.
    let file = CmdFile::write(root, &options).unwrap();
    std::mem::forget(file);
    assert!(target.exists());
    assert!(restore_stale(root).unwrap());
    assert_eq!(fs::read_to_string(&target).unwrap(), "-windowed\r\n");

    // A cmd.txt changed by someone else after the launch stays as it is.
    fs::remove_file(&target).unwrap();
    let file = CmdFile::write(root, &options).unwrap();
    fs::write(&target, "-stream gf2 cuba\r\n").unwrap();
    assert!(!file.restore().unwrap());
    assert_eq!(fs::read_to_string(&target).unwrap(), "-stream gf2 cuba\r\n");
    assert!(!root.join("_doctor_toolkit").join(WRITTEN).exists());

    // Bad options write nothing.
    fs::remove_file(&target).unwrap();
    assert!(CmdFile::write(root, &streams(&["cuba"])).is_err());
    assert!(!target.exists());
}
