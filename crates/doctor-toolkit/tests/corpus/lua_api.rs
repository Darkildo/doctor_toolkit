//! Stage 74: `data/gf2/lua_api.toml` (the functions the game gives to Lua)
//! against `godfather2.exe` and the scripts of the game.

use std::collections::BTreeSet;
use std::sync::Mutex;

use doctor_toolkit::game_types::lua_api::{LuaFunctionKind, lua_api};
use doctor_toolkit::resource::lua::opcodes::{Op, ins};
use doctor_toolkit::resource::lua::{Chunk, Constant, Proto};

const IMAGE_BASE: u32 = 0x40_0000;
/// `lua_pushstring` and `lua_pushcclosure` of the exe.
const PUSHSTRING: u32 = 0x0062_5CA0;
/// Registrations below it are code, above it entries of arrays in the data.
const DATA_START: u32 = 0x00D5_5000;
/// Globals the scripts of the game read and nobody sets: they are nil there
/// (`PlayEmx(name, 1, 1, yes, 0, 1)` passes nil for `yes`).
const UNSET: [&str; 7] = [
    "SFX_AMBIENT",
    "UI",
    "exterior",
    "flag",
    "palmasSingerIsSpawned",
    "set_protest_walla_quiet",
    "yes",
];

fn address(text: &str) -> u32 {
    u32::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

fn dword(exe: &[u8], va: u32) -> u32 {
    let at = (va - IMAGE_BASE) as usize;
    u32::from_le_bytes(exe[at..at + 4].try_into().unwrap())
}

fn c_string(exe: &[u8], va: u32) -> Option<&[u8]> {
    let at = va.checked_sub(IMAGE_BASE)? as usize;
    let rest = exe.get(at..)?;
    Some(&rest[..rest.iter().position(|&c| c == 0)?])
}

/// Every function of the data file is registered in the exe where the file
/// says: an entry `{name, function}` of an array, or a call of
/// `lua_pushstring` with the name pushed just before and the function
/// stored just after. (`research/lua_api.py check` also proves that the file
/// misses nothing.)
#[test]
fn the_lua_functions_are_registered_where_the_data_says() {
    let Some(dir) = super::game_dir() else { return };
    let exe = std::fs::read(dir.join("godfather2.exe")).unwrap();
    if exe.len() != 13_840_384 {
        eprintln!("skipped: godfather2.exe is not the 1.0.789.0 edition");
        return;
    }
    let api = lua_api();
    let mut checked = 0;
    for f in &api.functions {
        let name = f.full_name();
        if f.kind == LuaFunctionKind::Lua {
            // The embedded Lua text defines the function.
            let text = c_string(&exe, address(&f.source)).unwrap();
            let text = String::from_utf8_lossy(text);
            assert!(
                text.contains(&format!("function {}(", f.name))
                    || text.contains(&format!("function {}( ", f.name)),
                "{name}: not defined by the text at {}",
                f.source
            );
            checked += 1;
            continue;
        }
        let function = address(&f.implementation);
        for register in &f.register {
            let at = address(register);
            let file = (at - IMAGE_BASE) as usize;
            if at < DATA_START {
                // call lua_pushstring; the name is pushed within 24 bytes before.
                let target = at.wrapping_add(5).wrapping_add(dword(&exe, at + 1));
                assert_eq!(exe[file], 0xE8, "{name}: {register} is not a call");
                assert_eq!(
                    target, PUSHSTRING,
                    "{name}: {register} is not a call of lua_pushstring"
                );
                let named = (5..24).any(|back| {
                    exe[file - back] == 0x68
                        && c_string(&exe, dword(&exe, at - back as u32 + 1))
                            == Some(f.name.as_bytes())
                });
                assert!(named, "{name}: the name is not pushed before {register}");
                let after = &exe[file + 5..file + 125];
                assert!(
                    after
                        .windows(4)
                        .any(|w| u32::from_le_bytes(w.try_into().unwrap()) == function),
                    "{name}: {} is not stored after {register}",
                    f.implementation
                );
            } else {
                assert_eq!(
                    c_string(&exe, dword(&exe, at)),
                    Some(f.name.as_bytes()),
                    "{name}: no array entry at {register}"
                );
                assert_eq!(dword(&exe, at + 4), function, "{name}: {register}");
            }
            checked += 1;
        }
    }
    // 189 registered functions (EclipseLog twice) and 6 of the embedded Lua text.
    assert_eq!(checked, 196);
}

fn globals(p: &Proto, read: &mut BTreeSet<String>, written: &mut BTreeSet<String>) {
    for &i in &p.code {
        let set = match ins::op(i) {
            Some(Op::GetGlobal) => false,
            Some(Op::SetGlobal) => true,
            _ => continue,
        };
        if let Some(Constant::String(name)) = p.constants.get(ins::bx(i) as usize) {
            let name = String::from_utf8_lossy(name).into_owned();
            if set {
                written.insert(name);
            } else {
                read.insert(name);
            }
        }
    }
    for child in &p.protos {
        globals(child, read, written);
    }
}

/// Every global name the 30 scripts of the game read is a global one of them
/// sets, a function or table of the data file, or one of the names the exe
/// is known not to have.
#[test]
fn the_scripts_of_the_game_use_only_the_listed_functions() {
    let read = Mutex::new(BTreeSet::new());
    let written = Mutex::new(BTreeSet::new());
    let count = super::for_each_resource("lua", |_, data| {
        let chunk = Chunk::parse(data).map_err(|e| e.to_string())?;
        globals(
            &chunk.main,
            &mut read.lock().unwrap(),
            &mut written.lock().unwrap(),
        );
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 30);
    let api = lua_api();
    let mut known: BTreeSet<String> = written.into_inner().unwrap();
    known.extend(
        api.functions
            .iter()
            .filter(|f| f.owner.is_empty() || f.owner == "_G")
            .map(|f| f.name.clone()),
    );
    known.extend(api.tables.iter().map(|t| t.name.clone()));
    let missing: BTreeSet<String> = api.missing.iter().map(|m| m.name.clone()).collect();
    let read = read.into_inner().unwrap();
    let unknown: Vec<&str> = read
        .iter()
        .map(String::as_str)
        .filter(|n| !known.contains(*n) && !missing.contains(*n))
        .collect();
    assert_eq!(unknown, UNSET, "globals nobody defines");
    // The names of `[[missing]]` are really called and really not defined.
    for name in &missing {
        assert!(read.contains(name), "{name} is not used by the scripts");
        assert!(!known.contains(name), "{name} is defined");
    }
    // The functions of the game the scripts call.
    let used = api
        .functions
        .iter()
        .filter(|f| f.is_game_global() && read.contains(&f.name))
        .count();
    assert!(used >= 40, "{used}");
}
