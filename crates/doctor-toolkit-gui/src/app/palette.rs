//! The command palette (Ctrl+Shift+P): every command of the menus, recent
//! games, streams and, through the game index, resources by name, key, hash
//! or text. File → Open stream… (Ctrl+Shift+O) shows only the streams.

use std::path::PathBuf;

use egui::{Key, RichText};

use doctor_toolkit_mods::i18n::{tr, tr_in};

use super::commands::COMMANDS;
use super::{BrowserApp, Command, FileSel};
use crate::nav::Location;

/// Items listed at most.
const LIMIT: usize = 120;
/// Streams listed at most with the commands.
const FILE_LIMIT: usize = 12;
/// Resources looked up in the game index at most.
const RESOURCE_LIMIT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Commands, recent games, streams and resources.
    #[default]
    All,
    /// Streams and tables of contents.
    Streams,
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    mode: Mode,
    pub text: String,
    selected: usize,
    focus: bool,
    /// Resources found for a text, kept while the text stays the same.
    resources: Option<(String, Vec<(Location, String)>)>,
}

impl Palette {
    pub fn open(&mut self, mode: Mode) {
        *self = Self {
            open: true,
            mode,
            focus: true,
            ..Self::default()
        };
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Command(Command),
    Layout(super::layouts::LayoutId),
    Recent(PathBuf),
    File(FileSel),
    Resource(Location),
}

/// One line of the palette.
pub(super) struct Entry {
    item: Item,
    pub label: String,
    detail: String,
    score: i32,
}

/// How well `pattern` matches `text`: every word of the pattern must occur,
/// as a substring (better) or, when `scattered`, as letters in order. `None`
/// when it does not. Long texts (paths) should not match scattered letters:
/// almost every path has them.
pub fn fuzzy_score(pattern: &str, text: &str, scattered: bool) -> Option<i32> {
    let text = text.to_lowercase();
    let mut score = 0;
    for word in pattern.to_lowercase().split_whitespace() {
        if let Some(pos) = text.find(word) {
            score += 100 + 2 * word.len() as i32 - (pos as i32).min(50);
            let starts_word = pos == 0 || !text.as_bytes()[pos - 1].is_ascii_alphanumeric();
            if starts_word {
                score += 30;
            }
        } else if scattered {
            let mut rest = text.chars();
            let mut gaps = 0;
            for c in word.chars() {
                loop {
                    match rest.next() {
                        Some(t) if t == c => break,
                        Some(_) => gaps += 1,
                        None => return None,
                    }
                }
            }
            score += 40 - gaps.min(40);
        } else {
            return None;
        }
    }
    Some(score)
}

impl BrowserApp {
    pub(super) fn palette_entries(&mut self, ctx: &egui::Context) -> Vec<Entry> {
        let text = self.palette.text.trim().to_owned();
        let mut groups: Vec<Vec<Entry>> = Vec::new();
        let scored = |label: &str| fuzzy_score(&text, label, true);
        if self.palette.mode == Mode::All {
            let mut commands = Vec::new();
            for spec in COMMANDS {
                let command = spec.command;
                if command == Command::Palette || !self.enabled(command) {
                    continue;
                }
                let label = format!("{}: {}", tr(spec.menu.key()), command.label());
                // The English name finds a command in every language: the
                // guide and the community name commands in English.
                let english = format!(
                    "{}: {}",
                    tr_in("en", spec.menu.key()),
                    tr_in("en", spec.key)
                );
                if let Some(score) = scored(&label).max(scored(&english)) {
                    let detail = spec
                        .shortcuts
                        .first()
                        .map(|s| ctx.format_shortcut(s))
                        .unwrap_or_default();
                    commands.push(Entry {
                        item: Item::Command(command),
                        label,
                        detail,
                        score,
                    });
                }
            }
            groups.push(commands);
            // Layouts, built in and the user's (stage 52).
            let mut layouts = Vec::new();
            let ids = super::layouts::Builtin::ALL
                .into_iter()
                .map(super::layouts::LayoutId::Builtin)
                .chain(
                    self.layouts
                        .user
                        .iter()
                        .map(|n| super::layouts::LayoutId::User(n.name.clone())),
                );
            for id in ids {
                let label = format!("{}: {}: {}", tr("menu.view"), tr("layout.menu"), id.name());
                let english = format!("View: Layouts: {}", id.name());
                if let Some(score) = scored(&label).max(scored(&english)) {
                    let detail = if id == self.layouts.current {
                        tr("palette.current").to_owned()
                    } else {
                        String::new()
                    };
                    layouts.push(Entry {
                        item: Item::Layout(id),
                        label,
                        detail,
                        score,
                    });
                }
            }
            groups.push(layouts);
            let mut recent = Vec::new();
            for dir in &self.settings.recent_games {
                let label = format!("{}: {}", tr("palette.recent"), dir.display());
                if let Some(score) = scored(&label) {
                    recent.push(Entry {
                        item: Item::Recent(dir.clone()),
                        label,
                        detail: String::new(),
                        score,
                    });
                }
            }
            groups.push(recent);
        }
        if let Some(game) = &self.game
            && (self.palette.mode == Mode::Streams || !text.is_empty())
        {
            let files = game
                .tocs()
                .iter()
                .enumerate()
                .map(|(i, f)| (FileSel::Toc(i), f))
                .chain(
                    game.streams()
                        .iter()
                        .enumerate()
                        .map(|(i, f)| (FileSel::Stream(i), f)),
                );
            let mut list = Vec::new();
            for (sel, file) in files {
                if let Some(score) = fuzzy_score(&text, &file.rel_path, false) {
                    list.push(Entry {
                        item: Item::File(sel),
                        label: file.rel_path.clone(),
                        detail: super::human_size(file.len),
                        score,
                    });
                }
            }
            list.sort_by_key(|e| std::cmp::Reverse(e.score));
            if self.palette.mode == Mode::All {
                list.truncate(FILE_LIMIT);
            }
            groups.push(list);
        }
        if self.palette.mode == Mode::All && text.chars().count() >= 2 && self.nav.index.is_some() {
            if self
                .palette
                .resources
                .as_ref()
                .is_none_or(|(t, _)| *t != text)
            {
                let found = self.nav.quick_search(&text, RESOURCE_LIMIT);
                self.palette.resources = Some((text.clone(), found));
            }
            let found = &self.palette.resources.as_ref().expect("just set").1;
            groups.push(
                found
                    .iter()
                    .enumerate()
                    .map(|(i, (location, label))| Entry {
                        item: Item::Resource(*location),
                        label: label.clone(),
                        detail: tr("palette.resource").to_owned(),
                        score: -(i as i32),
                    })
                    .collect(),
            );
        }
        let mut out = Vec::new();
        for mut group in groups {
            // Stable: equal scores keep the menu or file order.
            group.sort_by_key(|e| std::cmp::Reverse(e.score));
            out.extend(group);
        }
        out.truncate(LIMIT);
        out
    }

    /// The palette window, when it is open.
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let entries = self.palette_entries(ctx);
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.palette.open = false;
            return;
        }
        if down && self.palette.selected + 1 < entries.len() {
            self.palette.selected += 1;
        }
        if up {
            self.palette.selected = self.palette.selected.saturating_sub(1);
        }
        self.palette.selected = self.palette.selected.min(entries.len().saturating_sub(1));
        let mut chosen = enter.then_some(self.palette.selected);
        let title = match self.palette.mode {
            Mode::All => tr("palette.title"),
            Mode::Streams => tr("palette.streams_title"),
        };
        let mut open = true;
        egui::Window::new(title)
            .id(egui::Id::unique("command-palette"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 48.0])
            .default_width(620.0)
            .show(ctx, |ui| {
                let hint = match self.palette.mode {
                    Mode::All => tr("palette.hint"),
                    Mode::Streams => tr("palette.streams_hint"),
                };
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette.text)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut self.palette.focus) {
                    response.request_focus();
                }
                if response.changed() {
                    self.palette.selected = 0;
                }
                if entries.is_empty() {
                    ui.label(RichText::new(tr("palette.nothing")).weak());
                    if self.game.is_some() && self.nav.index.is_none() {
                        ui.label(RichText::new(tr("palette.index_not_ready")).weak());
                    }
                }
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, entry) in entries.iter().enumerate() {
                            let selected = i == self.palette.selected;
                            let response = ui
                                .horizontal(|ui| {
                                    let r = ui.selectable_label(selected, &entry.label);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| ui.label(RichText::new(&entry.detail).weak()),
                                    );
                                    r
                                })
                                .inner;
                            if selected && (up || down) {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                chosen = Some(i);
                            }
                        }
                    });
            });
        if !open {
            self.palette.open = false;
        }
        if let Some(entry) = chosen.and_then(|i| entries.get(i)) {
            self.palette.open = false;
            self.run_palette_item(entry.item.clone(), ctx);
        }
    }

    fn run_palette_item(&mut self, item: Item, ctx: &egui::Context) {
        match item {
            Item::Command(command) => self.pending.push(command),
            Item::Layout(id) => self.dock_ops.push(super::dock::DockOp::ApplyLayout(id)),
            Item::Recent(dir) => self.open_game(dir, ctx),
            Item::File(sel) => self.open_file(sel, false),
            Item::Resource(location) => {
                self.nav.visit(location);
                self.open_location(location);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn substrings_beat_scattered_letters() {
        let exact = fuzzy_score("open", "File: Open game…", true).unwrap();
        let scattered = fuzzy_score("opgm", "File: Open game…", true).unwrap();
        assert!(exact > scattered);
        assert_eq!(fuzzy_score("xyz", "File: Open game…", true), None);
        assert_eq!(fuzzy_score("", "anything", true), Some(0));
    }

    #[test]
    fn every_word_must_match() {
        assert!(fuzzy_score("tools mods", "Tools: Mods", true).is_some());
        assert!(fuzzy_score("tools quit", "Tools: Mods", true).is_none());
        let start = fuzzy_score("front", "gf2/frontend.str", false).unwrap();
        let middle = fuzzy_score("tend", "gf2/frontend.str", false).unwrap();
        assert!(start > middle, "word starts rank first");
        assert_eq!(fuzzy_score("frnt", "gf2/frontend.str", false), None);
    }
}
