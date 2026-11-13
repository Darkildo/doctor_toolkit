//! String tables as CSV for spreadsheets (LibreOffice Calc, Excel).
//!
//! The first row names the columns: `hash`, then one column per language
//! (its code, or `text`, `text1`, … for tables without codes). Each further
//! row is one string: the hash as eight hexadecimal digits and the texts.
//! Fields with a separator, a quote or a line break are quoted (RFC 4180).
//! The reader also takes `;` and tab as separators (chosen by the header
//! row) and a UTF-8 byte order mark.

use crate::resource::lh2::StringTable;
use crate::{Error, Result};

/// Column names of a string table: its language codes, or `text`, `text1`, ….
pub fn columns(table: &StringTable) -> Vec<String> {
    if !table.languages.is_empty() {
        table.languages.clone()
    } else if table.strings.len() == 1 {
        vec!["text".to_owned()]
    } else {
        (0..table.strings.len())
            .map(|i| format!("text{i}"))
            .collect()
    }
}

/// The table as CSV text, rows separated by `\r\n` as RFC 4180 has it.
pub fn write_table(table: &StringTable) -> String {
    let mut out = String::new();
    let mut header = vec!["hash".to_owned()];
    header.extend(columns(table));
    write_row(&mut out, header.iter().map(String::as_str));
    for (row, hash) in table.hashes.iter().enumerate() {
        let hash = format!("{hash:08x}");
        let texts = table.strings.iter().map(|lang| lang[row].as_str());
        write_row(&mut out, std::iter::once(hash.as_str()).chain(texts));
    }
    out
}

fn write_row<'a>(out: &mut String, fields: impl Iterator<Item = &'a str>) {
    for (i, field) in fields.enumerate() {
        if i > 0 {
            out.push(',');
        }
        let quote = field.is_empty()
            || field.contains([',', '"', '\n', '\r', ';', '\t'])
            || field.starts_with(' ')
            || field.ends_with(' ');
        if quote {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push_str("\r\n");
}

/// Parses CSV text into rows of fields.
pub fn parse(text: &str) -> Result<Vec<Vec<String>>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let separator = separator(text);
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut quote_line = 0;
    let mut in_quotes = false;
    // The field was quoted and its closing quote seen.
    let mut after_quote = false;
    let mut row_started = false;
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => {
                    in_quotes = false;
                    after_quote = true;
                }
                c => {
                    if c == '\n' {
                        line += 1;
                    }
                    field.push(c);
                }
            }
            continue;
        }
        let end_row = match c {
            '"' if field.is_empty() && !after_quote => {
                in_quotes = true;
                quote_line = line;
                row_started = true;
                false
            }
            c if c == separator => {
                row.push(std::mem::take(&mut field));
                after_quote = false;
                row_started = true;
                false
            }
            '\r' if chars.peek() == Some(&'\n') => false,
            '\r' | '\n' => true,
            c => {
                if after_quote {
                    return Err(Error::InvalidArgument(format!(
                        "CSV line {line}: text after a closing quote"
                    )));
                }
                field.push(c);
                row_started = true;
                false
            }
        };
        if end_row {
            line += 1;
            if row_started {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            after_quote = false;
            row_started = false;
        }
    }
    if in_quotes {
        return Err(Error::InvalidArgument(format!(
            "CSV line {quote_line}: a quoted field is not closed"
        )));
    }
    if row_started {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

/// The separator the header row uses: `,`, else `;`, else a tab.
fn separator(text: &str) -> char {
    let first = text.lines().next().unwrap_or_default();
    [',', ';', '\t']
        .into_iter()
        .find(|&c| first.contains(c))
        .unwrap_or(',')
}

/// Applies a CSV file written by [`write_table`] (and edited) to `table`.
/// Every row must name a hash of the table and every language column must
/// be present; the order of rows and columns is free. Returns the number of
/// changed strings.
pub fn apply_table(table: &mut StringTable, bytes: &[u8]) -> Result<usize> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| Error::InvalidArgument(format!("the CSV file is not UTF-8: {e}")))?;
    let rows = parse(text)?;
    let Some((header, body)) = rows.split_first() else {
        return Err(Error::InvalidArgument("the CSV file is empty".into()));
    };
    let find = |name: &str| {
        header
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let hash_column = find("hash")
        .ok_or_else(|| Error::InvalidArgument("the CSV file has no `hash` column".into()))?;
    let mut language_columns = Vec::new();
    for name in columns(table) {
        let column = find(&name).ok_or_else(|| {
            Error::InvalidArgument(format!("the CSV file has no `{name}` column"))
        })?;
        language_columns.push(column);
    }
    let mut changed = 0;
    for (n, row) in body.iter().enumerate() {
        if row.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let line = n + 2;
        let text = row.get(hash_column).map(|h| h.trim()).unwrap_or_default();
        let hash = u32::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| {
            Error::InvalidArgument(format!("CSV row {line}: `{text}` is not a hash"))
        })?;
        let entry = table.find(hash).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "CSV row {line}: hash {hash:08x} is not in the table; adding strings is not supported"
            ))
        })?;
        for (lang, &column) in language_columns.iter().enumerate() {
            let value = row.get(column).map(String::as_str).unwrap_or_default();
            if table.strings[lang][entry] != value {
                table.strings[lang][entry] = value.to_owned();
                changed += 1;
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> StringTable {
        StringTable {
            languages: vec!["en".into(), "ru".into()],
            hashes: vec![0x0000_0001, 0x1234_5678, 0xdead_beef],
            strings: vec![
                vec![
                    "plain".into(),
                    String::new(),
                    "comma, \"quote\"\nnew line".into(),
                ],
                vec!["простой".into(), " spaced ".into(), "; tab\there".into()],
            ],
            version: 0,
            flags: 0,
        }
    }

    #[test]
    fn tables_round_trip() {
        let original = table();
        let text = write_table(&original);
        assert!(text.starts_with("hash,en,ru\r\n00000001,plain,простой\r\n"));
        let mut copy = original.clone();
        copy.strings[0][0] = "changed".into();
        assert_eq!(apply_table(&mut copy, text.as_bytes()).unwrap(), 1);
        assert_eq!(copy, original);
        // Unchanged text changes nothing.
        assert_eq!(apply_table(&mut copy, text.as_bytes()).unwrap(), 0);
    }

    #[test]
    fn spreadsheet_variants_are_read() {
        let mut t = table();
        // Semicolons, a byte order mark, other column order, LF line ends,
        // a blank line and only some rows.
        let text = "\u{feff}ru;hash;en\n\"x\";DEADBEEF;\"a;b\"\n\n";
        assert_eq!(apply_table(&mut t, text.as_bytes()).unwrap(), 2);
        assert_eq!(t.strings[0][2], "a;b");
        assert_eq!(t.strings[1][2], "x");
        assert_eq!(t.strings[0][0], "plain");
    }

    #[test]
    fn bad_files_are_refused() {
        let mut t = table();
        for (text, why) in [
            ("", "empty"),
            ("en,ru\n", "no `hash`"),
            ("hash,en\n", "no `ru`"),
            ("hash,en,ru\n99999999,a,b\n", "not in the table"),
            ("hash,en,ru\nzz,a,b\n", "not a hash"),
            ("hash,en,ru\n00000001,\"open,b\n", "not closed"),
            ("hash,en,ru\n00000001,\"a\"b,c\n", "after a closing quote"),
        ] {
            let error = apply_table(&mut t, text.as_bytes())
                .unwrap_err()
                .to_string();
            assert!(error.contains(why), "{text:?}: {error}");
        }
        assert_eq!(t, table());
    }

    #[test]
    fn quoted_fields() {
        let rows = parse("a,\"b\"\"c\",,\"\"\r\n\"x\ny\",z").unwrap();
        assert_eq!(rows, vec![vec!["a", "b\"c", "", ""], vec!["x\ny", "z"]]);
    }
}
