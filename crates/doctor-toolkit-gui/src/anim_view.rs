//! Animation banks (`bnk`): the clips with their frames, scheme and events,
//! named entries, a skeleton preview of the selected clip, JSON and glTF
//! export, and importing a glTF, FBX or USD animation over a clip or as a
//! new clip.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use doctor_toolkit::export::gltf::{NoAssets, SceneAnimation};
use doctor_toolkit::import::anim::{
    ClipTarget, ImportOptions, ImportSummary, import_clip, read_animations, retarget,
};
use doctor_toolkit::resource::anim::{
    AnimBank, BoneTrack, EventArg, FRAME_RATE, SourceAnimation, TYPE_BNK, bone_set, find_skeleton,
};
use doctor_toolkit::resource::anim_image::{BankImage, Entry};
use doctor_toolkit::resource::rcb::{Mat4, Rig, Skeleton, multiply};
use doctor_toolkit::scene::{self, ModelFormat};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, RichText, Sense, Stroke, Vec2};

use crate::render::{OrbitCamera, math};
use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

/// Clips of an animation bank.
pub struct AnimView {
    data: Vec<u8>,
    bank: AnimBank,
    entries: Vec<Entry>,
    looping: Vec<bool>,
    name: String,
    /// Resource key text, for labels.
    key: String,
    selected: usize,
    /// The bank's skeleton, from the game's rigs.
    skeleton: Option<Skeleton>,
    /// Rigs that hold the skeleton and the bone sets, for imports.
    rigs: Vec<Rig>,
    /// Bones of the bone set of each clip (by bone set hash).
    bone_sets: HashMap<u32, Vec<usize>>,
    preview: Option<Preview>,
    camera: Option<OrbitCamera>,
    import: Option<ImportDialog>,
    /// The graph window (stage 43).
    graph: Option<crate::anim_graph_view::GraphEditor>,
    /// Shown in a separate window.
    window: bool,
}

/// Tracks shown on the skeleton.
struct Preview {
    /// What is shown: a clip index of the bank, or `None` for an import.
    clip: Option<usize>,
    tracks: Vec<BoneTrack>,
    frames: usize,
    frame: f32,
    playing: bool,
    last: Instant,
}

/// The import window: a file read, its animation chosen and the target.
struct ImportDialog {
    path: PathBuf,
    animations: Vec<SourceAnimation>,
    chosen: usize,
    add: bool,
    name: String,
    /// 0 keep, 1 loop, 2 once.
    looping: usize,
    result: Option<Result<(Vec<u8>, ImportSummary), String>>,
}

pub const VIEW: ViewEntry = ViewEntry { id: "anim", load };

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    if source.type_id != TYPE_BNK {
        return None;
    }
    let data = source.data(source.index).ok()?;
    let bank = AnimBank::parse(&data).ok()?;
    let image = BankImage::parse(&data).ok();
    let rigs_all = source.assets.rigs();
    let skeleton = find_skeleton(rigs_all, bank.skeleton).cloned();
    let bone_sets = bank
        .clips
        .iter()
        .filter_map(|c| Some((c.bone_set, bone_set(rigs_all, c.bone_set)?.to_vec())))
        .collect();
    let rigs = rigs_all
        .iter()
        .filter(|r| r.skeletons.iter().any(|s| s.name_hash == bank.skeleton))
        .cloned()
        .collect();
    let mut view = AnimView {
        entries: image
            .as_ref()
            .map(BankImage::entry_list)
            .unwrap_or_default(),
        looping: image
            .as_ref()
            .map(|i| i.clips.iter().map(|c| c.looping()).collect())
            .unwrap_or_default(),
        key: source
            .key(source.index)
            .map(|k| k.to_string())
            .unwrap_or_default(),
        data,
        bank,
        name: format!("bnk_{:05}", source.index),
        selected: 0,
        skeleton,
        rigs,
        bone_sets,
        preview: None,
        camera: None,
        import: None,
        graph: None,
        window: false,
    };
    view.apply_preset();
    Some(Box::new(view))
}

impl AnimView {
    /// `DOCTOR_TOOLKIT_GUI_ANIM=clip=<n>,frame=<f>,import=<file.glb>[,add=<name>]`
    /// `[,graph[=<entry>][,node=<n>]]`: select and show a clip, stop at a
    /// frame, open the import window and preview the file, open the graph
    /// window on an entry and select a node (for screenshots without a user).
    fn apply_preset(&mut self) {
        let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_ANIM") else {
            return;
        };
        let mut frame = None;
        let mut import = None;
        let mut add = None;
        let mut graph = None;
        let mut node = None;
        for item in preset.split(',') {
            match item.split_once('=') {
                Some(("clip", v)) => {
                    if let Ok(c) = v.parse::<usize>()
                        && c < self.bank.clips.len()
                    {
                        self.selected = c;
                        self.preview_clip(c);
                    }
                }
                Some(("frame", v)) => frame = v.parse::<f32>().ok(),
                Some(("import", v)) => import = Some(PathBuf::from(v)),
                Some(("add", v)) => add = Some(v.to_owned()),
                Some(("graph", v)) => graph = Some(Some(v.to_owned())),
                Some(("node", v)) => node = v.parse::<usize>().ok(),
                None if item == "graph" => graph = Some(None),
                None if item == "window" => self.window = true,
                _ => {}
            }
        }
        if let Some(path) = import {
            let mut status = String::new();
            self.open_import(path, &mut status);
            if let (Some(dialog), Some(name)) = (&mut self.import, add) {
                dialog.add = true;
                dialog.name = name;
            }
            self.try_import();
        }
        if let Some(entry) = graph {
            let names = self.bank.clips.iter().map(|c| c.display_name()).collect();
            if let Ok(mut editor) = crate::anim_graph_view::GraphEditor::new(&self.data, names) {
                editor.preset(entry.as_deref(), node);
                self.graph = Some(editor);
            }
        }
        if let (Some(p), Some(f)) = (&mut self.preview, frame) {
            p.frame = f.min(p.frames.saturating_sub(1) as f32);
            p.playing = false;
        }
    }
}

/// Rotation (x, y, z, w) and translation as a column-major matrix.
fn trs(q: [f32; 4], t: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = q;
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// Joint positions of the skeleton posed by `tracks` at `frame` (bind pose
/// for bones without a track).
fn pose(skeleton: &Skeleton, tracks: &[BoneTrack], frame: usize) -> Vec<[f32; 3]> {
    let mut locals = skeleton.bind_locals();
    for t in tracks {
        if let (Some(l), Some(q), Some(p)) = (
            locals.get_mut(t.bone),
            t.rotations.get(frame).or(t.rotations.last()),
            t.translations.get(frame).or(t.translations.last()),
        ) {
            *l = trs(*q, *p);
        }
    }
    let mut globals: Vec<Mat4> = Vec::with_capacity(locals.len());
    for (i, bone) in skeleton.bones.iter().enumerate() {
        let g = match bone.parent.filter(|&p| p < i) {
            Some(p) => multiply(&globals[p], &locals[i]),
            None => locals[i],
        };
        globals.push(g);
    }
    globals.iter().map(|g| [g[12], g[13], g[14]]).collect()
}

impl AnimView {
    fn export_scene(&self, clip: usize) -> Result<scene::Scene, String> {
        let skeleton = self.skeleton.as_ref().ok_or(tr("anim.no_skeleton"))?;
        let clip = &self.bank.clips[clip];
        let bones = self
            .bone_sets
            .get(&clip.bone_set)
            .ok_or(tr("anim.no_bone_set"))?;
        let tracks = clip.decode(&self.data, bones).map_err(|e| e.to_string())?;
        let animation = SceneAnimation {
            name: clip.display_name(),
            skeleton,
            tracks,
            frame_rate: FRAME_RATE,
        };
        scene::game::build(&[], Some(skeleton), &[animation], &mut NoAssets)
            .map_err(|e| e.to_string())
    }

    /// Shows clip `index` on the skeleton.
    fn preview_clip(&mut self, index: usize) {
        let clip = &self.bank.clips[index];
        let Some(bones) = self.bone_sets.get(&clip.bone_set) else {
            self.preview = None;
            return;
        };
        self.preview = clip.decode(&self.data, bones).ok().map(|tracks| Preview {
            clip: Some(index),
            frames: clip.frames.max(1) as usize,
            tracks,
            frame: 0.0,
            playing: true,
            last: Instant::now(),
        });
    }

    /// Opens a file chosen by the user in the import window.
    fn open_import(&mut self, path: PathBuf, status: &mut String) {
        match read_animations(&path) {
            Ok(animations) => {
                let name = self.bank.clips[self.selected].display_name();
                self.import = Some(ImportDialog {
                    path,
                    animations,
                    chosen: 0,
                    add: false,
                    name,
                    looping: 0,
                    result: None,
                });
            }
            Err(e) => *status = trf("anim.cannot_read", &[("error", &e)]),
        }
    }

    fn target(&self, dialog: &ImportDialog) -> ClipTarget {
        if dialog.add {
            ClipTarget::Add {
                name: dialog.name.trim().to_owned(),
                entry: true,
                priority: 0,
                like: Some(self.selected),
            }
        } else {
            ClipTarget::Replace(self.selected)
        }
    }

    /// Runs the import without recording it; previews the result.
    fn try_import(&mut self) {
        let Some(dialog) = &self.import else { return };
        let options = ImportOptions {
            looping: match dialog.looping {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
            ..Default::default()
        };
        let source = &dialog.animations[dialog.chosen];
        let result = import_clip(
            &self.data,
            &self.rigs,
            source,
            &self.target(dialog),
            &options,
        )
        .map_err(|e| e.to_string());
        // Preview the tracks as fitted to the skeleton.
        if let (Ok((_, summary)), Some(skeleton)) = (&result, &self.skeleton)
            && let Some(bones) = bone_set(&self.rigs, summary.bone_set)
            && let Ok(fitted) = retarget(source, skeleton, bones, &HashMap::new())
        {
            self.preview = Some(Preview {
                clip: None,
                frames: fitted.frames,
                tracks: fitted.tracks,
                frame: 0.0,
                playing: true,
                last: Instant::now(),
            });
        }
        if let Some(dialog) = &mut self.import {
            dialog.result = Some(result);
        }
    }

    /// The import window; returns the data to record when the user applies.
    fn import_window(&mut self, ctx: &egui::Context) -> Option<(Vec<u8>, String)> {
        let mut open = true;
        let mut run = false;
        let mut apply = None;
        let clip_name = self.bank.clips[self.selected].display_name();
        let dialog = self.import.as_mut()?;
        egui::Window::new(tr("anim.import_title"))
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(dialog.path.display().to_string());
                if dialog.animations.len() > 1 {
                    egui::ComboBox::from_label(tr("anim.animation"))
                        .selected_text(&dialog.animations[dialog.chosen].name)
                        .show_ui(ui, |ui| {
                            for (i, a) in dialog.animations.iter().enumerate() {
                                ui.selectable_value(&mut dialog.chosen, i, &a.name);
                            }
                        });
                }
                let a = &dialog.animations[dialog.chosen];
                ui.label(trf(
                    "anim.source_info",
                    &[
                        ("frames", &trn("count.frames", a.frames as u64, &[])),
                        ("joints", &trn("anim.joints", a.joints.len() as u64, &[])),
                        ("rate", &FRAME_RATE),
                    ],
                ));
                ui.radio_value(
                    &mut dialog.add,
                    false,
                    trf("anim.replace_clip", &[("clip", &clip_name)]),
                );
                ui.horizontal(|ui| {
                    ui.radio_value(&mut dialog.add, true, tr("anim.add_clip"));
                    ui.add_enabled(
                        dialog.add,
                        egui::TextEdit::singleline(&mut dialog.name).desired_width(200.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(tr("anim.looping"));
                    ui.radio_value(&mut dialog.looping, 0, tr("anim.loop_keep"));
                    ui.radio_value(&mut dialog.looping, 1, tr("anim.loop_on"));
                    ui.radio_value(&mut dialog.looping, 2, tr("anim.loop_off"));
                });
                ui.horizontal(|ui| {
                    if ui.button(tr("anim.preview")).clicked() {
                        run = true;
                    }
                    let ready = matches!(dialog.result, Some(Ok(_)));
                    if ui
                        .add_enabled(ready, egui::Button::new(tr("anim.apply")))
                        .on_hover_text(tr("anim.apply_hint"))
                        .clicked()
                        && let Some(Ok((data, summary))) = &dialog.result
                    {
                        apply = Some((data.clone(), summary.clip.clone(), summary.mode.clone()));
                    }
                });
                match &dialog.result {
                    Some(Ok((_, s))) => {
                        ui.label(trf(
                            "anim.result",
                            &[
                                ("frames", &trn("count.frames", s.frames as u64, &[])),
                                ("bones", &trn("count.bones", s.bones as u64, &[])),
                                ("mapped", &s.mapped.len()),
                                ("angle", &format!("{:.4}", s.max_angle)),
                                ("bytes", &trn("count.bytes", s.streams_after as u64, &[])),
                            ],
                        ));
                        for w in &s.warnings {
                            ui.colored_label(Color32::YELLOW, w);
                        }
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                    None => {}
                }
            });
        if run {
            self.try_import();
        }
        if !open {
            self.import = None;
        }
        let (data, clip, mode) = apply?;
        self.import = None;
        let args: [(&str, &dyn std::fmt::Display); 2] = [("clip", &clip), ("bank", &self.key)];
        let label = if mode == "add" {
            trf("anim.add_label", &args)
        } else {
            trf("anim.import_label", &args)
        };
        Some((data, label))
    }

    /// The skeleton posed by the preview, drawn with the renderer's camera.
    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let Some(skeleton) = &self.skeleton else {
            return;
        };
        let Some(p) = &mut self.preview else {
            return;
        };
        let now = Instant::now();
        if p.playing {
            p.frame += now.duration_since(p.last).as_secs_f32() * FRAME_RATE;
            if p.frame >= p.frames as f32 {
                p.frame %= p.frames as f32;
            }
            ui.ctx().request_repaint();
        }
        p.last = now;
        ui.horizontal(|ui| {
            let label = if p.playing {
                tr("anim.pause")
            } else {
                tr("anim.play")
            };
            if ui.button(label).clicked() {
                p.playing = !p.playing;
            }
            let mut f = p.frame.floor() as usize;
            if ui
                .add(
                    egui::Slider::new(&mut f, 0..=p.frames.saturating_sub(1))
                        .text(tr("anim.frame")),
                )
                .changed()
            {
                p.frame = f as f32;
                p.playing = false;
            }
            ui.label(match p.clip {
                Some(c) => self.bank.clips[c].display_name(),
                None => tr("anim.imported").to_owned(),
            });
        });
        let positions = pose(skeleton, &p.tracks, p.frame.floor() as usize);
        let camera = self.camera.get_or_insert_with(|| {
            let mut bounds = math::Aabb::EMPTY;
            for q in pose(skeleton, &[], 0) {
                bounds.add_point(q);
            }
            let mut c = OrbitCamera::framing(&bounds);
            c.distance *= 1.15;
            c
        });
        let size = Vec2::new(
            ui.available_width(),
            300.0f32.min(ui.available_height().max(160.0)),
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        camera.handle_input(ui, &response);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_gray(28));
        let view = camera.view(rect.aspect_ratio());
        let screen: Vec<Option<Pos2>> = positions
            .iter()
            .map(|&q| {
                view.project(q)
                    .map(|(uv, _)| rect.min + Vec2::new(uv[0], uv[1]) * rect.size())
            })
            .collect();
        // A ground grid around the origin, 0.5 m apart.
        for k in -2..=2 {
            let v = k as f32 * 0.5;
            for (a, b) in [
                ([v, 0.0, -1.0], [v, 0.0, 1.0]),
                ([-1.0, 0.0, v], [1.0, 0.0, v]),
            ] {
                let (a, b) = (view.project(a), view.project(b));
                if let (Some((a, _)), Some((b, _))) = (a, b) {
                    painter.line_segment(
                        [
                            rect.min + Vec2::new(a[0], a[1]) * rect.size(),
                            rect.min + Vec2::new(b[0], b[1]) * rect.size(),
                        ],
                        Stroke::new(1.0, Color32::from_gray(60)),
                    );
                }
            }
        }
        let animated: std::collections::HashSet<usize> = p.tracks.iter().map(|t| t.bone).collect();
        for (i, bone) in skeleton.bones.iter().enumerate() {
            if let (Some(parent), Some(a)) = (bone.parent, screen[i])
                && let Some(b) = screen.get(parent).copied().flatten()
            {
                let color = if animated.contains(&i) {
                    Color32::from_rgb(255, 200, 90)
                } else {
                    Color32::from_gray(140)
                };
                painter.line_segment([b, a], Stroke::new(2.0, color));
            }
        }
    }
}

impl ResourceViewer for AnimView {
    fn show(&mut self, ui: &mut egui::Ui, cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        if !self.window {
            if ui.button(tr("anim.window")).clicked() {
                self.window = true;
            }
            return self.contents(ui, cx);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("anim.in_window")).weak());
            if ui.button(tr("anim.dock")).clicked() {
                self.window = false;
            }
        });
        let mut open = true;
        let mut action = None;
        egui::Window::new(trf("anim.window_title", &[("bank", &self.key)]))
            .id(egui::Id::unique("anim view window"))
            .open(&mut open)
            .default_size([1100.0, 860.0])
            .resizable(true)
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("anim window scroll")
                    .show(ui, |ui| action = self.contents(ui, cx));
            });
        if !open {
            self.window = false;
        }
        action
    }
}

impl AnimView {
    fn contents(&mut self, ui: &mut egui::Ui, cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        let bank = &self.bank;
        let frames = bank.clips.iter().map(|c| c.frames as u64).sum::<u64>();
        ui.strong(trf(
            if self.skeleton.is_some() {
                "anim.bank_info"
            } else {
                "anim.bank_info_no_skeleton"
            },
            &[
                ("clips", &trn("anim.clips", bank.clips.len() as u64, &[])),
                ("frames", &trn("count.frames", frames, &[])),
                ("skeleton", &format!("{:08x}", bank.skeleton)),
            ],
        ));
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("anim_clips")
            .max_height(200.0)
            .show(ui, |ui| {
                for (i, clip) in bank.clips.iter().enumerate() {
                    let text = format!(
                        "{:<40} {:>11}  {}  {}{}",
                        clip.display_name(),
                        trn("count.frames", clip.frames as u64, &[]),
                        trf("anim.scheme", &[("scheme", &clip.scheme)]),
                        trn("anim.events", clip.events.len() as u64, &[]),
                        if self.looping.get(i) == Some(&true) {
                            tr("anim.loop_mark")
                        } else {
                            ""
                        }
                    );
                    if ui
                        .selectable_label(self.selected == i, RichText::new(text).monospace())
                        .clicked()
                    {
                        clicked = Some(i);
                    }
                }
            });
        if let Some(i) = clicked {
            self.selected = i;
            self.preview_clip(i);
        }
        if ui
            .button(tr("anim.graph"))
            .on_hover_text(tr("anim.graph_hint"))
            .clicked()
        {
            let names = self.bank.clips.iter().map(|c| c.display_name()).collect();
            match crate::anim_graph_view::GraphEditor::new(&self.data, names) {
                Ok(editor) => self.graph = Some(editor),
                Err(e) => *cx.status = trf("anim.graph_error", &[("error", &e)]),
            }
        }
        if !self.entries.is_empty() {
            egui::CollapsingHeader::new(trf("anim.entries", &[("count", &self.entries.len())]))
                .id_salt("anim_entries")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("anim_entry_list")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for e in &self.entries {
                                let clips: Vec<String> = e
                                    .clips
                                    .iter()
                                    .map(|&c| self.bank.clips[c].display_name())
                                    .collect();
                                ui.monospace(format!("{:<32} {}", e.name, clips.join(", ")));
                            }
                        });
                });
        }
        let mut export = None;
        let mut pick_file = false;
        let mut show_clip = false;
        let previewing = self.preview.is_some();
        if let Some(clip) = self.bank.clips.get(self.selected) {
            ui.horizontal(|ui| {
                if ui.button(tr("anim.export_json")).clicked() {
                    export = Some(false);
                }
                let can = self.skeleton.is_some() && self.bone_sets.contains_key(&clip.bone_set);
                if ui
                    .add_enabled(can, egui::Button::new(tr("anim.export")))
                    .on_hover_text(tr("anim.export_hint"))
                    .clicked()
                {
                    export = Some(true);
                }
                if ui
                    .add_enabled(can, egui::Button::new(tr("anim.import_clip")))
                    .on_hover_text(tr("anim.import_hint"))
                    .clicked()
                {
                    pick_file = true;
                }
                if !previewing && can && ui.button(tr("anim.show_clip")).clicked() {
                    show_clip = true;
                }
            });
            if !clip.events.is_empty() {
                ui.label(RichText::new(tr("anim.events_title")).weak());
                egui::ScrollArea::vertical()
                    .id_salt("anim_events")
                    .max_height(100.0)
                    .show(ui, |ui| {
                        for event in &clip.events {
                            let args: Vec<String> = event
                                .args
                                .iter()
                                .map(|a| match a {
                                    EventArg::Int(v) => v.to_string(),
                                    EventArg::Float(v) => format!("{v}"),
                                    EventArg::Text(t) => format!("{t:?}"),
                                })
                                .collect();
                            ui.monospace(format!(
                                "{:7.1}  {}  {}",
                                event.frame,
                                event.name,
                                args.join(" ")
                            ));
                        }
                    });
            }
        }
        if show_clip {
            self.preview_clip(self.selected);
        }
        self.preview_panel(ui);
        if pick_file
            && let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    "glTF, FBX, USD",
                    &["glb", "gltf", "fbx", "usd", "usda", "usdc", "usdz"],
                )
                .pick_file()
        {
            self.open_import(path, cx.status);
        }
        let mut action = None;
        if let Some((data, label)) = self.import_window(ui.ctx()) {
            action = Some(ViewAction::SaveData { data, label });
        }
        if let Some(editor) = &mut self.graph {
            if let Some((data, label)) = editor.window(ui.ctx(), &self.key) {
                action = Some(ViewAction::SaveData { data, label });
            }
            if !editor.open {
                self.graph = None;
            }
        }
        match export {
            Some(false) => {
                let file = format!("{}.json", self.name);
                *cx.status = match serde_json::to_vec_pretty(&self.bank) {
                    Err(e) => trf("resource.cannot_export", &[("error", &e)]),
                    Ok(json) => save(&file, &json),
                };
            }
            Some(true) => {
                let clip = &self.bank.clips[self.selected];
                let file = format!("{}_{}.glb", self.name, clip.display_name());
                *cx.status = match self.export_scene(self.selected) {
                    Err(e) => trf("resource.cannot_export", &[("error", &e)]),
                    Ok(built) => save_scene(&file, &built),
                };
            }
            None => {}
        }
        action
    }
}

fn save_scene(file: &str, built: &scene::Scene) -> String {
    let mut dialog = rfd::FileDialog::new().set_file_name(file);
    for format in ModelFormat::WRITABLE
        .into_iter()
        .filter(|f| f.has_skeletons())
    {
        dialog = dialog.add_filter(format.label(), &[format.extension()]);
    }
    match dialog.save_file() {
        None => tr("resource.export_cancelled").to_owned(),
        Some(path) => {
            let format = ModelFormat::from_path(&path)
                .filter(|f| f.has_skeletons())
                .unwrap_or(ModelFormat::Glb);
            match scene::write_file(built, &path, Some(format)) {
                Ok(_) => trf("resource.saved", &[("path", &path.display())]),
                Err(e) => trf(
                    "resource.cannot_save",
                    &[("path", &path.display()), ("error", &e)],
                ),
            }
        }
    }
}

fn save(file: &str, data: &[u8]) -> String {
    match rfd::FileDialog::new().set_file_name(file).save_file() {
        None => tr("resource.export_cancelled").to_owned(),
        Some(path) => match std::fs::write(&path, data) {
            Ok(()) => trf("resource.saved", &[("path", &path.display())]),
            Err(e) => trf(
                "resource.cannot_save",
                &[("path", &path.display()), ("error", &e)],
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_pose_matrix() {
        let m = trs([0.0, 0.0, 0.0, 1.0], [1.0, 2.0, 3.0]);
        assert_eq!(m[0], 1.0);
        assert_eq!(m[5], 1.0);
        assert_eq!(m[10], 1.0);
        assert_eq!([m[12], m[13], m[14]], [1.0, 2.0, 3.0]);
        // 90° about Z maps X to Y.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let r = trs([0.0, 0.0, s, s], [0.0; 3]);
        assert!((r[1] - 1.0).abs() < 1e-6 && r[0].abs() < 1e-6);
    }
}
