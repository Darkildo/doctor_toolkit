//! Interface strings by key, in English and Russian.
//!
//! Text shown by the programs is looked up with [`tr`] (or [`trf`] for text
//! with `{name}` placeholders, [`trn`] for text that depends on a number)
//! under a dotted key such as `menu.file`. The tables are compiled in from
//! `i18n/<code>.toml`, TOML files whose tables give the first part of the key:
//!
//! ```toml
//! [menu]
//! file = "File"                                   # tr("menu.file")
//! [files]
//! count = { one = "{n} file", other = "{n} files" } # trn("files.count", n, &[])
//! ```
//!
//! A table of only `one`, `few`, `many` and `other` is a plural: the form is
//! chosen by the number and the rules of the language (English: `one` for 1,
//! `other` for the rest; Russian: `one` for 1, 21, 101…, `few` for 2–4,
//! 22–24…, `many` for 0, 5–20, 25…).
//!
//! English is the fallback: a key a translation lacks shows the English
//! text, and a key missing everywhere shows as the key itself, so a typo is
//! visible instead of empty. The language may change at any time with
//! [`set_language`] or [`choose_language`]; the next frame draws in it.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// English, the fallback for every key.
const EN: &str = include_str!("../i18n/en.toml");
const RU: &str = include_str!("../i18n/ru.toml");

/// Languages with a compiled-in table: code, own name and file contents.
/// English comes first.
const LANGUAGES: &[(&str, &str, &str)] = &[("en", "English", EN), ("ru", "Русский", RU)];

/// Plural forms in the order of [`PluralForm`].
pub const PLURAL_FORMS: [&str; 4] = ["one", "few", "many", "other"];

/// A plural form of a language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralForm {
    One,
    Few,
    Many,
    Other,
}

impl PluralForm {
    pub fn name(self) -> &'static str {
        PLURAL_FORMS[self as usize]
    }
}

/// One language: plain strings and plurals (`key` → form → text).
#[derive(Debug, Default)]
pub struct Table {
    pub strings: HashMap<String, String>,
    pub plurals: HashMap<String, HashMap<String, String>>,
}

static TABLES: OnceLock<Vec<Table>> = OnceLock::new();
/// Index into [`LANGUAGES`] of the language in use.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

fn tables() -> &'static [Table] {
    TABLES.get_or_init(|| LANGUAGES.iter().map(|(_, _, text)| parse(text)).collect())
}

/// Codes and own names (`"ru"`, `"Русский"`) of the languages that have a table.
pub fn languages() -> impl Iterator<Item = (&'static str, &'static str)> {
    LANGUAGES.iter().map(|(code, name, _)| (*code, *name))
}

/// The own name of language `code`, or `code` itself.
pub fn language_name(code: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map_or(code, |(_, name, _)| name)
}

/// Chooses the language at start: `DOCTOR_TOOLKIT_LANG` if set, else `code`
/// (`"en"`, `"ru"`; `None` or empty for the system language), else the
/// language of the system. Returns the language in use.
pub fn set_language(code: Option<&str>) -> &'static str {
    let wanted = std::env::var("DOCTOR_TOOLKIT_LANG")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| code.filter(|c| !c.is_empty()).map(str::to_owned));
    choose(wanted)
}

/// Switches the language while the program runs: `code`, or the language of
/// the system for `None` or empty. Returns the language in use.
pub fn choose_language(code: Option<&str>) -> &'static str {
    choose(code.filter(|c| !c.is_empty()).map(str::to_owned))
}

fn choose(wanted: Option<String>) -> &'static str {
    let wanted = wanted.or_else(system_language).unwrap_or_default();
    let index = find_language(&wanted).unwrap_or(0);
    CURRENT.store(index, Ordering::Relaxed);
    LANGUAGES[index].0
}

/// The index of the language whose code `wanted` starts with (`ru_RU` → `ru`).
fn find_language(wanted: &str) -> Option<usize> {
    let wanted = wanted.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .position(|(code, _, _)| wanted.starts_with(code))
}

/// The language in use.
pub fn language() -> &'static str {
    LANGUAGES[CURRENT.load(Ordering::Relaxed)].0
}

/// The language of the system: on Windows the user's locale, elsewhere the
/// language part of `LC_ALL`, `LC_MESSAGES` or `LANG` (`ru_RU.UTF-8` → `ru`).
pub fn system_language() -> Option<String> {
    #[cfg(windows)]
    if let Some(code) = windows_locale() {
        return Some(code);
    }
    from_environment(|name| std::env::var(name).ok())
}

fn from_environment(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| var(name))
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX" && !v.starts_with("C."))
        .map(|v| {
            v.split(['_', '-', '.', '@'])
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        })
}

/// The user's default locale name (`ru-RU`) through `GetUserDefaultLocaleName`.
#[cfg(windows)]
fn windows_locale() -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, length: i32) -> i32;
    }
    let mut buffer = [0u16; 85];
    // SAFETY: the buffer is writable and its length is passed; the function
    // writes at most that many UTF-16 units including the terminating zero.
    let written = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if written <= 1 {
        return None;
    }
    let name = String::from_utf16_lossy(&buffer[..written as usize - 1]);
    name.split(['-', '_']).next().map(str::to_ascii_lowercase)
}

/// A list to choose the language from: the system's or one of [`languages`].
/// `setting` is the code saved in the settings, empty for the system's; a
/// choice switches the interface at once. Returns whether it changed.
pub fn language_choice(ui: &mut egui::Ui, setting: &mut String) -> bool {
    let system = system_language()
        .and_then(|code| find_language(&code))
        .map_or("English", |i| LANGUAGES[i].1);
    let system = trf("prefs.language_system", &[("language", &system)]);
    let selected = if setting.is_empty() {
        system.clone()
    } else {
        language_name(setting).to_owned()
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt("language")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(setting, String::new(), system)
                .changed();
            for (code, name) in languages() {
                changed |= ui
                    .selectable_value(setting, code.to_owned(), name)
                    .changed();
            }
        })
        .response
        .on_hover_text(tr("prefs.language_hint"));
    if changed {
        choose_language(Some(setting.as_str()));
    }
    changed
}

/// Parses a table file; a broken file gives an empty table and a log line.
pub fn parse(text: &str) -> Table {
    let mut out = Table::default();
    match text.parse::<toml::Table>() {
        Ok(table) => flatten("", &table, &mut out),
        Err(error) => doctor_toolkit::report::log(&format!("bad translation table: {error}")),
    }
    out
}

fn is_plural(table: &toml::Table) -> bool {
    !table.is_empty()
        && table
            .iter()
            .all(|(k, v)| PLURAL_FORMS.contains(&k.as_str()) && v.is_str())
}

fn flatten(prefix: &str, table: &toml::Table, out: &mut Table) {
    for (key, value) in table {
        let full = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::String(text) => {
                out.strings.insert(full, text.clone());
            }
            toml::Value::Table(inner) if is_plural(inner) => {
                let forms = inner
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect();
                out.plurals.insert(full, forms);
            }
            toml::Value::Table(inner) => flatten(&full, inner, out),
            _ => {}
        }
    }
}

/// The plural form of `n` in language `code`.
pub fn plural_form(code: &str, n: u64) -> PluralForm {
    match code {
        "ru" => {
            let (d10, d100) = (n % 10, n % 100);
            if d10 == 1 && d100 != 11 {
                PluralForm::One
            } else if (2..=4).contains(&d10) && !(12..=14).contains(&d100) {
                PluralForm::Few
            } else {
                PluralForm::Many
            }
        }
        _ => {
            if n == 1 {
                PluralForm::One
            } else {
                PluralForm::Other
            }
        }
    }
}

/// The forms a plural must have in language `code`.
pub fn plural_forms(code: &str) -> &'static [PluralForm] {
    match code {
        "ru" => &[PluralForm::One, PluralForm::Few, PluralForm::Many],
        _ => &[PluralForm::One, PluralForm::Other],
    }
}

/// The text of `key` in the interface language.
pub fn tr(key: &str) -> &'static str {
    lookup(CURRENT.load(Ordering::Relaxed), key)
}

/// The text of `key` in language `code` (English for an unknown code).
pub fn tr_in(code: &str, key: &str) -> &'static str {
    lookup(find_language(code).unwrap_or(0), key)
}

fn lookup(index: usize, key: &str) -> &'static str {
    let tables = tables();
    match tables[index]
        .strings
        .get(key)
        .or_else(|| tables[0].strings.get(key))
    {
        Some(text) => text.as_str(),
        None => missing(key),
    }
}

/// A missing key shows itself; leaked once so the result stays 'static.
fn missing(key: &str) -> &'static str {
    static MISSING: OnceLock<std::sync::Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut missing = MISSING
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    missing
        .entry(key.to_owned())
        .or_insert_with(|| Box::leak(key.to_owned().into_boxed_str()))
}

/// The text of `key` with `{name}` placeholders replaced by `args`.
pub fn trf(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(tr(key), args)
}

/// The plural `key` in the form for `n`, with `{n}` replaced by `n` and the
/// other placeholders by `args`.
pub fn trn(key: &str, n: u64, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(&plural(CURRENT.load(Ordering::Relaxed), key, n), args)
}

/// [`trn`] in language `code`.
pub fn trn_in(code: &str, key: &str, n: u64, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(&plural(find_language(code).unwrap_or(0), key, n), args)
}

fn plural(index: usize, key: &str, n: u64) -> String {
    let tables = tables();
    let text = [index, 0].iter().find_map(|&i| {
        let forms = tables[i].plurals.get(key)?;
        let form = plural_form(LANGUAGES[i].0, n);
        forms
            .get(form.name())
            .or_else(|| forms.get("other"))
            .or_else(|| forms.get("many"))
    });
    text.map_or_else(|| missing(key), String::as_str)
        .replace("{n}", &n.to_string())
}

fn fill(text: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut text = text.to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

/// Whether the English table has `key`, plain or plural; for tests of the callers.
pub fn has_key(key: &str) -> bool {
    let english = &tables()[0];
    english.strings.contains_key(key) || english.plurals.contains_key(key)
}

/// The compiled-in table of language `code`; for tests.
pub fn table(code: &str) -> Option<&'static Table> {
    let index = LANGUAGES.iter().position(|(c, _, _)| *c == code)?;
    Some(&tables()[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_table_parses() {
        let table = parse(EN);
        assert!(table.strings.len() > 20, "{} keys", table.strings.len());
        assert_eq!(
            table.strings.get("menu.file").map(String::as_str),
            Some("File")
        );
    }

    #[test]
    fn lookup_and_fallback() {
        // Tests run in parallel: they read other languages with `tr_in` and
        // leave the global language English.
        assert_eq!(tr("menu.file"), "File");
        assert_eq!(tr("no.such.key"), "no.such.key");
        assert_eq!(
            trf("status.opened_game", &[("name", &"GF2"), ("streams", &3)]),
            tr("status.opened_game")
                .replace("{name}", "GF2")
                .replace("{streams}", "3")
        );
        assert_eq!(trn("count.files", 1, &[]), "1 file");
        assert_eq!(trn("count.files", 3, &[]), "3 files");
        assert_eq!(tr_in("ru", "menu.file"), "Файл");
        assert_eq!(trn_in("ru", "count.files", 1, &[]), "1 файл");
        assert_eq!(trn_in("ru", "count.files", 3, &[]), "3 файла");
        assert_eq!(trn_in("ru", "count.files", 11, &[]), "11 файлов");
        assert_eq!(trn_in("ru", "count.files", 21, &[]), "21 файл");
        assert_eq!(trn_in("ru", "count.files", 0, &[]), "0 файлов");
        assert_eq!(tr_in("ru", "no.such.key"), "no.such.key");
        assert_eq!(tr_in("de", "menu.file"), "File");
        // English is the default and the fallback of an unknown code.
        assert_eq!(choose_language(Some("de")), "en");
        assert_eq!(language(), "en");
    }

    #[test]
    fn plural_rules() {
        use PluralForm::*;
        let ru = |n| plural_form("ru", n);
        for (n, form) in [
            (0, Many),
            (1, One),
            (2, Few),
            (4, Few),
            (5, Many),
            (11, Many),
            (12, Many),
            (14, Many),
            (20, Many),
            (21, One),
            (22, Few),
            (25, Many),
            (101, One),
            (111, Many),
            (112, Many),
            (122, Few),
            (1001, One),
        ] {
            assert_eq!(ru(n), form, "{n}");
        }
        assert_eq!(plural_form("en", 0), Other);
        assert_eq!(plural_form("en", 1), One);
        assert_eq!(plural_form("en", 21), Other);
    }

    #[test]
    fn placeholders_and_plurals_parse() {
        let table = parse("[a]\nb = \"x {n} y\"\nc = { one = \"1\", other = \"2\" }\n");
        assert_eq!(table.strings["a.b"], "x {n} y");
        assert_eq!(table.plurals["a.c"]["other"], "2");
        assert!(!table.strings.contains_key("a.c.one"));
    }

    #[test]
    fn system_language_from_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            from_environment(env(&[("LANG", "ru_RU.UTF-8")])).as_deref(),
            Some("ru")
        );
        assert_eq!(
            from_environment(env(&[("LC_ALL", "C"), ("LANG", "en_GB.UTF-8")])).as_deref(),
            Some("en")
        );
        assert_eq!(
            from_environment(env(&[("LC_MESSAGES", "ru_UA"), ("LANG", "en_US")])).as_deref(),
            Some("ru")
        );
        assert_eq!(from_environment(env(&[("LANG", "C.UTF-8")])), None);
        assert_eq!(find_language("ru-RU"), Some(1));
        assert_eq!(find_language("fr"), None);
    }
}
