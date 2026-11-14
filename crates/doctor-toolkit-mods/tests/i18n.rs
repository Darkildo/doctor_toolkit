//! The translation tables agree with each other and with the code: every
//! key of `en.toml` is in `ru.toml` and back, with the same placeholders and
//! the plural forms of the language, and every key the code asks for with a
//! literal exists.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use doctor_toolkit_mods::i18n::{Table, plural_forms, table};

/// `{name}` placeholders of a text.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('}') {
            let name = &rest[..end];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                out.insert(name.to_owned());
            }
            rest = &rest[end + 1..];
        }
    }
    out
}

fn plural_placeholders(forms: &HashMap<String, String>) -> BTreeSet<String> {
    forms.values().flat_map(|t| placeholders(t)).collect()
}

fn tables() -> (&'static Table, &'static Table) {
    (table("en").unwrap(), table("ru").unwrap())
}

#[test]
fn both_tables_have_the_same_keys() {
    let (en, ru) = tables();
    let en_keys: BTreeSet<_> = en.strings.keys().collect();
    let ru_keys: BTreeSet<_> = ru.strings.keys().collect();
    let only_en: Vec<_> = en_keys.difference(&ru_keys).collect();
    let only_ru: Vec<_> = ru_keys.difference(&en_keys).collect();
    assert!(only_en.is_empty(), "not translated: {only_en:?}");
    assert!(only_ru.is_empty(), "not in en.toml: {only_ru:?}");
    let en_plurals: BTreeSet<_> = en.plurals.keys().collect();
    let ru_plurals: BTreeSet<_> = ru.plurals.keys().collect();
    let only_en: Vec<_> = en_plurals.difference(&ru_plurals).collect();
    let only_ru: Vec<_> = ru_plurals.difference(&en_plurals).collect();
    assert!(only_en.is_empty(), "plurals not translated: {only_en:?}");
    assert!(only_ru.is_empty(), "plurals not in en.toml: {only_ru:?}");
    assert!(en.strings.len() > 500, "{} strings", en.strings.len());
}

#[test]
fn placeholders_agree() {
    let (en, ru) = tables();
    let mut wrong = Vec::new();
    for (key, text) in &en.strings {
        if let Some(other) = ru.strings.get(key)
            && placeholders(text) != placeholders(other)
        {
            wrong.push(format!("{key}: {text:?} / {other:?}"));
        }
    }
    for (key, forms) in &en.plurals {
        if let Some(other) = ru.plurals.get(key)
            && plural_placeholders(forms) != plural_placeholders(other)
        {
            wrong.push(format!("{key}: {forms:?} / {other:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn plurals_have_the_forms_of_their_language() {
    for code in ["en", "ru"] {
        let table = table(code).unwrap();
        let wanted: BTreeSet<&str> = plural_forms(code).iter().map(|f| f.name()).collect();
        for (key, forms) in &table.plurals {
            let have: BTreeSet<&str> = forms.keys().map(String::as_str).collect();
            assert_eq!(have, wanted, "{code}: {key}");
            for text in forms.values() {
                assert!(text.contains("{n}"), "{code}: {key}: {text:?} lacks {{n}}");
            }
        }
    }
}

#[test]
fn texts_are_not_empty_and_russian_is_russian() {
    let (en, ru) = tables();
    for (key, text) in en.strings.iter().chain(&ru.strings) {
        assert!(!text.trim().is_empty(), "{key} is empty");
    }
    // A Russian text without a Cyrillic letter is most likely untranslated;
    // names, units and symbols are listed here. Placeholders do not count.
    let words = |text: &str| {
        let mut out = String::new();
        let mut depth = 0;
        for c in text.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    };
    let same: Vec<_> = ru
        .strings
        .iter()
        .filter(|(key, text)| {
            words(text).chars().any(|c| c.is_ascii_alphabetic())
                && !text.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c))
                && en.strings.get(*key) == Some(*text)
                && !KEPT_IN_ENGLISH.contains(&key.as_str())
        })
        .map(|(key, text)| format!("{key} = {text:?}"))
        .collect();
    assert!(same.is_empty(), "untranslated?\n{}", same.join("\n"));
}

/// Keys whose Russian text is the English one on purpose: names of formats,
/// programs and libraries, units.
const KEPT_IN_ENGLISH: &[&str] = &[
    "prefs.renderer_wgpu",
    "prefs.renderer_glow",
    "play.dll_overrides",
    "script.lua",
    "browse.col_id",
    "mods_app.title",
    "mods_app.powered_by",
];

/// Source files of both programs.
fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    let mut stack = vec![
        root.join("doctor-toolkit-gui/src"),
        root.join("doctor-toolkit-mods/src"),
    ];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs") {
                out.push(path);
            }
        }
    }
    out
}

/// `(function, key, line)` of every `tr("…")`, `trf("…"` and `trn("…"`
/// call with a literal key.
fn calls(source: &str) -> Vec<(&'static str, String, usize)> {
    let mut out = Vec::new();
    for name in ["tr", "trf", "trn"] {
        let pattern = format!("{name}(");
        let mut from = 0;
        while let Some(found) = source[from..].find(&pattern) {
            let at = from + found;
            from = at + pattern.len();
            let before = source[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let rest = source[from..].trim_start();
            let Some(rest) = rest.strip_prefix('"') else {
                continue;
            };
            let Some(end) = rest.find('"') else { continue };
            let line = source[..at].matches('\n').count() + 1;
            let name = match name {
                "tr" => "tr",
                "trf" => "trf",
                _ => "trn",
            };
            out.push((name, rest[..end].to_owned(), line));
        }
    }
    out
}

#[test]
fn every_literal_key_exists() {
    let (en, _) = tables();
    let mut wrong = Vec::new();
    let mut seen = 0;
    for path in sources() {
        let source = std::fs::read_to_string(&path).unwrap();
        for (function, key, line) in calls(&source) {
            seen += 1;
            let ok = match function {
                "trn" => en.plurals.contains_key(&key),
                _ => en.strings.contains_key(&key),
            };
            if !ok {
                wrong.push(format!("{}:{line}: {function}(\"{key}\")", path.display()));
            }
        }
    }
    assert!(seen > 500, "only {seen} calls found");
    assert!(
        wrong.is_empty(),
        "keys missing in en.toml:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn calls_are_found() {
    let source = "a(tr(\"x.y\")); b.trf(\n    \"x.z\", &[]); attr(\"no\"); trn(\"x.n\", 1, &[])";
    let found: Vec<_> = calls(source).into_iter().map(|(f, k, _)| (f, k)).collect();
    assert_eq!(
        found,
        [
            ("tr", "x.y".to_owned()),
            ("trf", "x.z".to_owned()),
            ("trn", "x.n".to_owned())
        ]
    );
}
