//! Lua 5.0 compiler and decompiler against reference chunks.
//!
//! `tests/data/lua/*.luac` were compiled from the `.lua` files next to them by
//! `luac` of Lua 5.0.3 built with `LUA_NUMBER=float`, 4-byte `size_t` and
//! instructions (the format of The Godfather II), as `luac -o x.luac x.lua`
//! (chunk name `@x.lua`).

use std::path::PathBuf;

use doctor_toolkit::resource::lua::{
    Chunk, Comparison, compile, compile_for_game, decompile, round_trip,
};

fn data(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/lua")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

const SAMPLES: [&str; 2] = ["constructs", "edge_cases"];

#[test]
fn compiler_matches_reference_luac() {
    for name in SAMPLES {
        let source = data(&format!("{name}.lua"));
        let reference = data(&format!("{name}.luac"));
        let chunk = compile(&source, format!("@{name}.lua").as_bytes(), 4).unwrap();
        assert_eq!(chunk.to_bytes(), reference, "{name}");
    }
}

#[test]
fn reference_chunks_parse_and_write_back() {
    for name in SAMPLES {
        let reference = data(&format!("{name}.luac"));
        let chunk = Chunk::parse(&reference).unwrap();
        assert_eq!(chunk.number_size, 4);
        assert_eq!(chunk.to_bytes(), reference, "{name}");
    }
}

#[test]
fn decompiled_samples_compile_to_the_same_code() {
    for name in SAMPLES {
        let chunk = Chunk::parse(&data(&format!("{name}.luac"))).unwrap();
        let rt = round_trip(&chunk);
        assert!(
            rt.decompiled.errors.is_empty(),
            "{name}: {:?}",
            rt.decompiled.errors
        );
        assert!(
            matches!(rt.comparison, Comparison::Identical | Comparison::SameCode),
            "{name}: {}\n{}",
            rt.comparison,
            rt.decompiled.source
        );
    }
}

#[test]
fn small_programs_round_trip_byte_identical() {
    let programs = [
        "local a = 1\nprint(a)\n",
        "x = { 1, 2, n = 3 }\n",
        "local function f(n)\n  if n < 2 then return 1 end\n  return n * f(n - 1)\nend\nprint(f(5))\n",
        "for i = 1, 3 do\n  print(i)\nend\n",
        "local t = {}\nfor k, v in pairs(t) do\n  t[k] = v and 1 or 2\nend\n",
        "while x do\n  x = x - 1\nend\n",
        "repeat\n  y = y + 1\nuntil y > 10\n",
        "local s = a == b\nlocal n = not s\n",
        // The jump over `elseif`/`else` has the line of the inner `end`
        // (aud_pc_car.lua, mus_underscore.lua of the game).
        "if a then\n  if b then\n    x = 0\n\n  end\nelseif c then\n  y = 1\nend\n",
        "if a then\n  if b then\n    x = 1\n  else\n    x = 2\n\n  end\nelse\nend\nz = 3\n",
    ];
    // The flush of 32 items is coded at the comma after the 32nd: the 33rd
    // item stays on its own line (mus_underscore_content_mia.lua).
    let long_table = format!(
        "t = {{\n{}}}\n",
        (1..=40).map(|i| format!("  {i}0,\n")).collect::<String>()
    );
    for source in programs.into_iter().chain([long_table.as_str()]) {
        let chunk = compile(source.as_bytes(), b"=test", 4).unwrap();
        let out = decompile(&chunk);
        assert!(out.errors.is_empty(), "{source}: {:?}", out.errors);
        let again = compile(out.source.as_bytes(), b"=test", 4).unwrap();
        assert_eq!(
            again.to_bytes(),
            chunk.to_bytes(),
            "{source}\n---\n{}",
            out.source
        );
    }
}

#[test]
fn numbers_are_float() {
    let chunk = Chunk::parse(&compile_for_game(b"x = 0.1 + 1e10 + -0.5", b"=n").unwrap()).unwrap();
    let out = decompile(&chunk).source;
    assert!(out.contains("0.1") && out.contains("-0.5"), "{out}");
    let again = compile_for_game(out.as_bytes(), b"=n").unwrap();
    assert_eq!(again, chunk.to_bytes());
}

#[test]
fn syntax_errors_carry_the_line() {
    let err = compile(b"local a = 1\n\nif a then\n  x = = 2\nend\n", b"=e", 4).unwrap_err();
    assert_eq!(err.line, 4);
    assert!(err.message.contains("unexpected symbol"), "{err}");
    let err = compile(b"for i = 1, 2 do\nprint(i)\n", b"=e", 4).unwrap_err();
    assert_eq!(err.line, 3, "the end of the file");
    assert!(err.message.contains("`end' expected"), "{err}");
    let err = compile(b"x = 'unfinished\n", b"=e", 4).unwrap_err();
    assert_eq!(err.line, 1);
    let err = compile(b"break\n", b"=e", 4).unwrap_err();
    assert!(err.message.contains("no loop to break"), "{err}");
}
