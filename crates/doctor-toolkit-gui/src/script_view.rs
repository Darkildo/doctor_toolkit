//! Editor for Lua scripts (`lua`): decompiled source with syntax colours,
//! compile errors at their line, and saving into the workspace.

use doctor_toolkit::resource::lua::{self, Chunk, Comparison, CompileError, TYPE_LUA, opcodes};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, TextStyle};

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

pub const VIEW: ViewEntry = ViewEntry { id: "script", load };

pub struct ScriptView {
    state: Result<Script, String>,
}

struct Script {
    chunk: Chunk,
    /// The source as decompiled from the current data.
    decompiled: String,
    /// How the decompiled source compiles compared with the data.
    round_trip: Comparison,
    /// The data differs from the game's original (an edit in the workspace).
    edited: bool,
    source: String,
    /// Result of compiling `source`, refreshed when it changes.
    check: Result<Vec<u8>, CompileError>,
    checked: String,
    listing: Option<String>,
    show_listing: bool,
    /// Error line the editor last scrolled to.
    scrolled_to: Option<u32>,
}

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    if source.type_id != TYPE_LUA {
        return None;
    }
    let data = source.data(source.index);
    let original = source
        .edited(source.index)
        .and_then(|_| source.original(source.index));
    let mut view = ScriptView::from_data(data, original);
    // Self-test: `DOCTOR_TOOLKIT_GUI_LUA=<file.lua>` puts that text in the editor.
    if let Some(path) = std::env::var_os("DOCTOR_TOOLKIT_GUI_LUA")
        && let Ok(script) = &mut view.state
        && let Ok(text) = std::fs::read_to_string(path)
    {
        script.source = text;
    }
    Some(Box::new(view))
}

impl ResourceViewer for ScriptView {
    /// The editor fills its tab (stage 56).
    fn tab_body(&self) -> crate::views::TabBody {
        crate::views::TabBody::Fill
    }

    fn show(&mut self, ui: &mut egui::Ui, cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        self.show_editor(ui, cx.status)
    }

    fn hides_hex(&self) -> bool {
        true
    }

    fn has_unsaved_edits(&self) -> bool {
        self.state.as_ref().is_ok_and(|s| s.source != s.decompiled)
    }
}

impl ScriptView {
    /// A view of `data`; `original` is the game's version when `data` is an edit.
    fn from_data(data: Result<Vec<u8>, String>, original: Option<Vec<u8>>) -> Self {
        let state = data.and_then(|data| {
            let chunk = Chunk::parse(&data).map_err(|e| e.to_string())?;
            let rt = lua::round_trip(&chunk);
            let source = rt.decompiled.source.clone();
            let check = rt
                .recompiled
                .as_ref()
                .map(Chunk::to_bytes)
                .map_err(Clone::clone);
            Ok(Script {
                edited: original.is_some_and(|o| o != data),
                chunk,
                decompiled: source.clone(),
                round_trip: rt.comparison,
                checked: source.clone(),
                source,
                check,
                listing: None,
                show_listing: false,
                scrolled_to: None,
            })
        });
        Self { state }
    }

    fn show_editor(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<ViewAction> {
        let script = match &mut self.state {
            Ok(script) => script,
            Err(error) => {
                ui.colored_label(Color32::RED, trf("lua.cannot_read", &[("error", &error)]));
                return None;
            }
        };
        script.refresh_check();
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong(script.chunk.source());
            ui.label(
                RichText::new(trf(
                    "lua.info",
                    &[
                        ("size", &script.chunk.number_size),
                        (
                            "functions",
                            &trn(
                                "lua.functions",
                                script.chunk.main.function_count() as u64,
                                &[],
                            ),
                        ),
                        (
                            "instructions",
                            &trn(
                                "lua.instructions",
                                script.chunk.main.instruction_count() as u64,
                                &[],
                            ),
                        ),
                    ],
                ))
                .weak(),
            );
        });
        ui.horizontal_wrapped(|ui| {
            let (text, colour) = match &script.round_trip {
                Comparison::Identical => (tr("lua.identical"), Color32::GREEN),
                Comparison::SameCode => (tr("lua.same_code"), Color32::LIGHT_GREEN),
                Comparison::Different(_) => (tr("lua.different"), Color32::YELLOW),
            };
            ui.colored_label(colour, text);
            if let Comparison::Different(why) = &script.round_trip {
                ui.label(RichText::new(why).weak());
            }
            if script.edited {
                ui.colored_label(crate::changes_view::PENDING, tr("lua.edited"));
            }
        });
        let unsaved = script.source != script.decompiled;
        ui.horizontal(|ui| {
            let can_save = unsaved && script.check.is_ok();
            if ui
                .add_enabled(can_save, egui::Button::new(tr("lua.save")))
                .on_hover_text(tr("lua.save_hint"))
                .clicked()
                && let Ok(data) = &script.check
            {
                let name = lua::file_name(&script.chunk.source()).to_owned();
                action = Some(ViewAction::SaveData {
                    data: data.clone(),
                    label: trf("lua.edit_label", &[("name", &name)]),
                });
            }
            if ui
                .add_enabled(unsaved, egui::Button::new(tr("lua.revert")))
                .on_hover_text(tr("lua.revert_hint"))
                .clicked()
            {
                script.source = script.decompiled.clone();
            }
            if ui.button(tr("lua.export")).clicked() {
                let name = lua::file_name(&script.chunk.source()).to_owned();
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(if name.is_empty() { "script.lua" } else { &name })
                    .add_filter(tr("lua.filter"), &["lua"])
                    .save_file()
                {
                    *status = match std::fs::write(&path, &script.source) {
                        Ok(()) => trf("resource.saved", &[("path", &path.display())]),
                        Err(e) => trf(
                            "resource.cannot_save",
                            &[("path", &path.display()), ("error", &e)],
                        ),
                    };
                }
            }
            if ui.button(tr("lua.load")).clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter(tr("lua.filter"), &["lua"])
                    .pick_file()
            {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        script.source = String::from_utf8_lossy(&bytes).into_owned();
                        *status = trf("lua.loaded", &[("path", &path.display())]);
                    }
                    Err(e) => {
                        *status = trf(
                            "lua.cannot_read_file",
                            &[("path", &path.display()), ("error", &e)],
                        )
                    }
                }
            }
            ui.toggle_value(&mut script.show_listing, tr("lua.bytecode"));
        });
        let error_line = match &script.check {
            Ok(_) => {
                if unsaved {
                    ui.colored_label(Color32::LIGHT_GREEN, tr("lua.compiles"));
                }
                None
            }
            Err(e) => {
                ui.colored_label(
                    Color32::RED,
                    trf("lua.error", &[("line", &e.line), ("message", &e.message)]),
                );
                Some(e.line)
            }
        };
        ui.separator();
        if script.show_listing {
            let listing = script
                .listing
                .get_or_insert_with(|| opcodes::listing(&script.chunk.main));
            egui::ScrollArea::both()
                .id_salt("lua_listing")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(listing.as_str()).monospace()).extend());
                });
            return action;
        }
        // Bring a new error into view once.
        let scroll_to = error_line.filter(|&l| script.scrolled_to != Some(l));
        script.scrolled_to = error_line;
        editor(ui, &mut script.source, error_line, scroll_to);
        action
    }
}

impl Script {
    fn refresh_check(&mut self) {
        if self.source == self.checked {
            return;
        }
        let name = self.chunk.main.source.clone().unwrap_or_default();
        self.check = lua::compile(self.source.as_bytes(), &name, self.chunk.number_size)
            .map(|c| c.to_bytes());
        self.checked = self.source.clone();
    }
}

/// A code editor with line numbers; `error_line` is highlighted.
fn editor(ui: &mut egui::Ui, source: &mut String, error_line: Option<u32>, scroll_to: Option<u32>) {
    let font = TextStyle::Monospace.resolve(ui.style());
    let lines = source.lines().count().max(1) + usize::from(source.ends_with('\n'));
    let dark = ui.visuals().dark_mode;
    // Wide enough for the longest line (no wrapping) and at least the panel.
    let longest = source.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
    let char_width = ui.ctx().fonts_mut(|f| f.glyph_width(&font, '0'));
    let width = (longest + 2.0) * char_width;
    let width = width.max(ui.available_width() - 60.0);
    let mut area = egui::ScrollArea::both()
        .id_salt("lua_source")
        .auto_shrink([false, false]);
    if let Some(line) = scroll_to {
        let row = ui.ctx().fonts_mut(|f| f.row_height(&font));
        area = area.vertical_scroll_offset(line.saturating_sub(5) as f32 * row);
    }
    area.show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let numbers: String = (1..=lines).map(|n| format!("{n:>5}\n")).collect();
            ui.add(
                egui::Label::new(
                    RichText::new(numbers)
                        .font(font.clone())
                        .color(ui.visuals().weak_text_color()),
                )
                .selectable(false),
            );
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
                let job = highlight(text.as_str(), &font, dark, error_line);
                ui.ctx().fonts_mut(|f| f.layout_job(job))
            };
            ui.add(
                egui::TextEdit::multiline(source)
                    .code_editor()
                    .desired_width(width)
                    .desired_rows(lines)
                    .lock_focus(true)
                    .margin(egui::Margin::ZERO)
                    .layouter(&mut layouter),
            );
        });
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Token {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
}

const KEYWORDS: [&str; 21] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Splits Lua source into coloured runs (a simple scanner, not the compiler's lexer).
fn tokens(text: &str) -> Vec<(usize, usize, Token)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let long_end = |from: usize| text[from..].find("]]").map_or(b.len(), |p| from + p + 2);
    while i < b.len() {
        let start = i;
        let c = b[i];
        let kind = if c == b'-' && b.get(i + 1) == Some(&b'-') {
            i = if text[i + 2..].starts_with("[[") {
                long_end(i + 4)
            } else {
                text[i..].find('\n').map_or(b.len(), |p| i + p)
            };
            Token::Comment
        } else if c == b'[' && b.get(i + 1) == Some(&b'[') {
            i = long_end(i + 2);
            Token::String
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            Token::String
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            Token::Number
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if KEYWORDS.contains(&&text[start..i]) {
                Token::Keyword
            } else {
                Token::Plain
            }
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            Token::Plain
        };
        out.push((start, i.min(b.len()), kind));
    }
    out
}

fn highlight(text: &str, font: &FontId, dark: bool, error_line: Option<u32>) -> LayoutJob {
    let colour = |t: Token| match (t, dark) {
        (Token::Plain, true) => Color32::from_gray(220),
        (Token::Plain, false) => Color32::from_gray(30),
        (Token::Keyword, true) => Color32::from_rgb(197, 134, 192),
        (Token::Keyword, false) => Color32::from_rgb(140, 40, 150),
        (Token::String, true) => Color32::from_rgb(206, 145, 120),
        (Token::String, false) => Color32::from_rgb(160, 60, 20),
        (Token::Number, true) => Color32::from_rgb(181, 206, 168),
        (Token::Number, false) => Color32::from_rgb(30, 120, 60),
        (Token::Comment, true) => Color32::from_rgb(106, 153, 85),
        (Token::Comment, false) => Color32::from_rgb(80, 120, 60),
    };
    // Byte range of the line with the error.
    let error_range = error_line.and_then(|line| {
        let mut start = 0;
        for (n, l) in text.split_inclusive('\n').enumerate() {
            if n + 1 == line as usize {
                return Some(start..start + l.len());
            }
            start += l.len();
        }
        None
    });
    let mut job = LayoutJob::default();
    for (start, end, kind) in tokens(text) {
        let mut cuts = vec![start, end];
        if let Some(r) = &error_range {
            for p in [r.start, r.end] {
                if p > start && p < end {
                    cuts.push(p);
                }
            }
        }
        cuts.sort_unstable();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a == b {
                continue;
            }
            let mut format = TextFormat::simple(font.clone(), colour(kind));
            if error_range.as_ref().is_some_and(|r| r.contains(&a)) {
                format.background = Color32::from_rgba_unmultiplied(220, 40, 40, 70);
            }
            job.append(&text[a..b], 0.0, format);
        }
    }
    job.wrap.max_width = f32::INFINITY;
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_covers_the_text() {
        let text = "local s = \"a\\\"b\" -- note\nx = [[long]] + 0.5 end";
        let toks = tokens(text);
        assert_eq!(toks.first().map(|t| t.0), Some(0));
        assert_eq!(toks.last().map(|t| t.1), Some(text.len()));
        for w in toks.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
        let kinds: Vec<Token> = toks.iter().map(|t| t.2).collect();
        assert!(kinds.contains(&Token::Keyword));
        assert!(kinds.contains(&Token::String));
        assert!(kinds.contains(&Token::Comment));
        assert!(kinds.contains(&Token::Number));
    }
}
