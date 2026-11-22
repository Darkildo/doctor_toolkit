//! The menu on screen: a panel in the colours of the game's menus (as the
//! mod manager has them), drawn by `doctor_overlay`. Sizes are for 1280×720
//! and scaled by the height of the frame.

use doctor_overlay::{Color, Face, Frame};

use crate::menu::{Kind, Menu, What};
use crate::text::{Lang, T};

const COAL: Color = Color::rgb(0x15, 0x12, 0x0c);
const OXBLOOD: Color = Color::rgb(0x4c, 0x00, 0x00);
const RUST: Color = Color::rgb(0xb5, 0x3a, 0x00);
const MUSTARD: Color = Color::rgb(0xab, 0xa4, 0x08);
const OLIVE: Color = Color::rgb(0x8f, 0x9a, 0x5a);
const BONE: Color = Color::rgb(0xe7, 0xdf, 0xcb);
const BONE_DIM: Color = Color::rgb(0xa9, 0x9d, 0x82);
const ASH: Color = Color::rgb(0x6a, 0x60, 0x4c);
const INK: Color = Color::rgb(0x1b, 0x16, 0x04);

/// A value as the menu shows it: `1`, `1.5`, `0.25`.
pub fn number(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A sum of money as the game's languages write it: `10 000` in Russian,
/// `10,000` in English.
pub fn money(v: i64, lang: Lang) -> String {
    let digits = v.unsigned_abs().to_string();
    let sep = match lang {
        Lang::Ru => ' ',
        Lang::En => ',',
    };
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    if v < 0 { format!("-{out}") } else { out }
}

/// A position as the menu shows it: tenths of a metre, no "-0.0".
pub fn coords(xyz: [f32; 3]) -> String {
    let tenth = |v: f32| {
        let r = (v * 10.0).round() / 10.0;
        if r == 0.0 { 0.0 } else { r }
    };
    let [x, y, z] = xyz.map(tenth);
    format!("{x:.1}  {y:.1}  {z:.1}")
}

/// Splits `text` into lines no wider than `max` (by words; a word wider
/// than `max` gets a line of its own).
pub fn wrap(text: &str, max: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && measure(&candidate) > max {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Draws the menu; `status` — a line under it (the result of an action).
pub fn draw(
    f: &mut Frame<'_>,
    menu: &Menu,
    lang: Lang,
    status: Option<&str>,
    info: &dyn Fn(What) -> Option<String>,
) {
    let s = f.scale;
    let (page, cursor) = menu.here();
    let x = 40.0 * s;
    let y = 40.0 * s;
    let w = 560.0 * s;
    let pad = 22.0 * s;
    let item_px = 25.0 * s;
    let row = f.line_height(Face::Text, item_px) + 6.0 * s;
    let root = menu.stack.len() <= 1;
    let head = if root { 62.0 } else { 84.0 } * s;
    let hint_px = 15.0 * s;
    let hint_line = 21.0 * s;
    let hint = wrap(T::Hint.get(lang), w - 2.0 * pad, |t| {
        f.measure(Face::Text, hint_px, t)
    });
    let foot =
        48.0 * s + hint_line * hint.len() as f32 + if status.is_some() { 30.0 * s } else { 0.0 };
    let h = head + row * page.items.len() as f32 + foot;

    f.rect(x, y, w, h, COAL.alpha(238));
    f.frame_rect(x, y, w, h, 3.0 * s, OXBLOOD);
    f.rect(x, y + head - 6.0 * s, w, 2.0 * s, RUST);

    let mut ty = y + 12.0 * s;
    if !root {
        f.text(
            Face::Text,
            x + pad,
            ty,
            15.0 * s,
            BONE_DIM,
            T::Title.get(lang),
        );
        ty += 20.0 * s;
    }
    f.text(
        Face::Title,
        x + pad,
        ty,
        32.0 * s,
        MUSTARD,
        &page.title.get(lang).to_uppercase(),
    );

    let mut line = y + head + 4.0 * s;
    for (i, item) in page.items.iter().enumerate() {
        let chosen = i == cursor;
        if chosen {
            f.rect(
                x + 10.0 * s,
                line - 2.0 * s,
                w - 20.0 * s,
                row - 2.0 * s,
                MUSTARD,
            );
        }
        let label_color = match (chosen, item.ready) {
            (true, _) => INK,
            (false, true) => BONE,
            (false, false) => ASH,
        };
        let label = item.text.get(lang);
        let lw = f.text(Face::Text, x + pad, line, item_px, label_color, label);
        if item.saves {
            let c = if chosen { INK } else { RUST };
            f.text(Face::Text, x + pad + lw + 6.0 * s, line, item_px, c, "•");
        }
        let value = match item.kind {
            _ if !item.ready => {
                Some((T::Soon.get(lang).to_owned(), if chosen { INK } else { ASH }))
            }
            Kind::Page => Some(("»".to_owned(), if chosen { INK } else { BONE_DIM })),
            Kind::Action => None,
            Kind::Toggle(on) => Some((
                if on { T::On } else { T::Off }.get(lang).to_owned(),
                match (chosen, on) {
                    (true, _) => INK,
                    (false, true) => OLIVE,
                    (false, false) => BONE_DIM,
                },
            )),
            Kind::Number { value, .. } => Some((number(value), if chosen { INK } else { BONE })),
            Kind::Info => info(item.what).map(|t| (t, if chosen { INK } else { BONE_DIM })),
        };
        if let Some((text, color)) = value {
            let px = if item.ready { item_px } else { 18.0 * s };
            let vw = f.measure(Face::Text, px, &text);
            let vy = line + (item_px - px) * 0.7;
            f.text(Face::Text, x + w - pad - vw, vy, px, color, &text);
        }
        line += row;
    }

    let mut fy = line + 14.0 * s;
    f.rect(x + pad, fy - 8.0 * s, w - 2.0 * pad, 1.0, OXBLOOD);
    if let Some(status) = status {
        f.text(Face::Text, x + pad, fy, 20.0 * s, MUSTARD, status);
        fy += 30.0 * s;
    }
    for line in &hint {
        f.text(Face::Text, x + pad, fy, hint_px, BONE_DIM, line);
        fy += hint_line;
    }
    f.text(
        Face::Text,
        x + pad,
        fy + 1.0 * s,
        hint_px,
        RUST,
        T::SavesNote.get(lang),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_has_the_separators_of_the_language() {
        assert_eq!(money(10_000, Lang::Ru), "10 000");
        assert_eq!(money(1_234_567, Lang::En), "1,234,567");
        assert_eq!(money(999, Lang::En), "999");
        assert_eq!(money(-5_000, Lang::Ru), "-5 000");
    }

    #[test]
    fn long_lines_wrap_by_words() {
        let measure = |t: &str| t.chars().count() as f32;
        assert_eq!(wrap("aa bb cc", 5.0, measure), vec!["aa bb", "cc"]);
        assert_eq!(wrap("aa bb cc", 100.0, measure), vec!["aa bb cc"]);
        assert_eq!(wrap("longword x", 3.0, measure), vec!["longword", "x"]);
        assert!(wrap("", 3.0, measure).is_empty());
    }

    #[test]
    fn coordinates_have_no_negative_zero() {
        assert_eq!(coords([-1.02e-7, 12.34, -0.04]), "0.0  12.3  0.0");
        assert_eq!(coords([-3.06, 0.0, 1.0]), "-3.1  0.0  1.0");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(number(1.0), "1");
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(12.0), "12");
    }
}
