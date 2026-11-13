//! Virtualised hex view: only the visible rows are laid out, so resources of
//! any size scroll smoothly.

use std::fmt::Write as _;

const BYTES_PER_ROW: usize = 16;

/// Formats one row: offset, hex bytes and printable ASCII.
pub fn format_row(data: &[u8], row: usize) -> String {
    let start = row * BYTES_PER_ROW;
    let bytes = &data[start..(start + BYTES_PER_ROW).min(data.len())];
    let mut line = format!("{start:08x}  ");
    for i in 0..BYTES_PER_ROW {
        match bytes.get(i) {
            Some(b) => {
                let _ = write!(line, "{b:02x} ");
            }
            None => line.push_str("   "),
        }
        if i == 7 {
            line.push(' ');
        }
    }
    line.push(' ');
    line.extend(bytes.iter().map(|&b| {
        if b.is_ascii_graphic() || b == b' ' {
            b as char
        } else {
            '.'
        }
    }));
    line
}

/// Shows `data` as a scrollable hex dump.
pub fn show(ui: &mut egui::Ui, id_salt: egui::IdSalt, data: &[u8]) {
    let rows = data.len().div_ceil(BYTES_PER_ROW);
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::both()
        .id_salt(id_salt)
        .auto_shrink(false)
        .show_rows(ui, row_height, rows, |ui, range| {
            for row in range {
                ui.add(
                    egui::Label::new(egui::RichText::new(format_row(data, row)).monospace())
                        .extend(),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_full_and_partial_rows() {
        let data: Vec<u8> = (0x41..0x41 + 20).collect();
        assert_eq!(
            format_row(&data, 0),
            "00000000  41 42 43 44 45 46 47 48  49 4a 4b 4c 4d 4e 4f 50  ABCDEFGHIJKLMNOP"
        );
        assert_eq!(
            format_row(&data, 1),
            "00000010  51 52 53 54                                       QRST"
        );
        let row = format_row(&[0, 0x7f, b' '], 0);
        assert!(row.starts_with("00000000  00 7f 20    "));
        assert!(row.ends_with("  .. "), "{row:?}");
        assert_eq!(row.len(), format_row(&data, 0).len() - 13);
    }
}
