//! Editing the world while the game runs (stage 47, protocol 6): moving,
//! turning, hiding and showing SimGroup objects, with a journal the module
//! keeps for undo (`docs/analysis/live-link.md` §15).
//!
//! The module changes an object through the game's own methods of
//! `Entity` (vtable `0x00E32858`): its matrix through `+0x28`
//! (`0x00470E30`, what the game calls with a whole matrix), hiding and
//! showing through the count of `+0x6C`/`+0x70` (`0x00470460`,
//! `0x00470490`) — the game's own deactivation, which takes the model off
//! the screen and the collision out of the physical world. An object whose
//! class replaces one of these methods is refused ([`ObjectState::unsupported`]).
//!
//! Every change is an [`ObjectEdit`] with the value before and after; the
//! [`EditJournal`] here is shared by the module and the fake one of the
//! tests, so both undo alike.

use crate::Guid;
use crate::proto::DecodeError;
use crate::wire::{Reader, Writer};

/// A 4×4 matrix as SimGroup (`CMD_LoadMatrix`) and the game keep it: rows
/// right, up, forward, translation; the last column 0, 0, 0, 1. The rows
/// carry the scale.
pub type Matrix = [f32; 16];

/// At most this many edits in the module's journal: the oldest goes.
pub const MAX_EDITS: usize = 512;

/// The state of one object, as the module sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectState {
    pub guid: Guid,
    /// Hash and name of its class (read from the code of its vtable's `As`,
    /// `0` and empty when the class has no `As` of its own).
    pub class: u32,
    pub class_name: String,
    /// Its matrix at `+0xD0`, when that is a world matrix.
    pub matrix: Option<Matrix>,
    /// `Entity`'s count of deactivations (`+0x161`; 0: in the world, seen
    /// and colliding). `None`: not an object whose hiding the module knows.
    pub inactive: Option<u8>,
    /// Whether it has a model (`+0x130`: the instance of its metamodel —
    /// parts, collision).
    pub has_model: bool,
    /// Whether its listener takes messages (bit 0 of `+0x40`).
    pub listening: bool,
    /// Whether the link hid it (the last [`EditKind::Shown`] edit of the
    /// journal for it says hidden).
    pub hidden_by_link: bool,
    /// Why the module does not move or hide it; empty when it does.
    pub unsupported: String,
}

impl ObjectState {
    /// Whether the module moves and hides it.
    pub fn supported(&self) -> bool {
        self.unsupported.is_empty()
    }

    /// Whether it is in the world now (seen and colliding).
    pub fn shown(&self) -> Option<bool> {
        self.inactive.map(|n| n == 0)
    }

    /// Its position, when it has a matrix.
    pub fn position(&self) -> Option<[f32; 3]> {
        self.matrix.map(|m| [m[12], m[13], m[14]])
    }
}

/// What an edit changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EditKind {
    /// The matrix (place, turn, scale): [`EditValue::Matrix`].
    Transform = 1,
    /// Whether it is in the world, seen and colliding: [`EditValue::Flag`].
    Shown = 2,
    /// Whether its listener takes messages (its logic):
    /// [`EditValue::Flag`].
    Listening = 3,
}

impl EditKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Transform,
            2 => Self::Shown,
            3 => Self::Listening,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Transform => "transform",
            Self::Shown => "shown",
            Self::Listening => "listening",
        }
    }
}

/// A value before or after an edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditValue {
    Matrix(Matrix),
    Flag(bool),
}

impl EditValue {
    pub fn matrix(&self) -> Option<&Matrix> {
        match self {
            Self::Matrix(m) => Some(m),
            Self::Flag(_) => None,
        }
    }

    pub fn flag(&self) -> Option<bool> {
        match self {
            Self::Flag(f) => Some(*f),
            Self::Matrix(_) => None,
        }
    }
}

/// One change the link made in the game.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectEdit {
    /// Number in the module's journal (from 1, never reused while the game
    /// runs).
    pub id: u32,
    pub guid: Guid,
    pub kind: EditKind,
    pub before: EditValue,
    pub after: EditValue,
    /// What happened, for people (`moved 1.50 m`, `hidden`).
    pub note: String,
}

impl ObjectEdit {
    /// The edit that undoes this one: the same object and kind, values
    /// swapped.
    pub fn inverse(&self) -> (EditValue, EditValue) {
        (self.after, self.before)
    }
}

/// The edits the link made and has not undone, oldest first.
#[derive(Debug, Clone, Default)]
pub struct EditJournal {
    next: u32,
    edits: Vec<ObjectEdit>,
}

impl EditJournal {
    /// An empty journal (for a `static`).
    pub const fn new() -> Self {
        Self {
            next: 0,
            edits: Vec::new(),
        }
    }

    /// Records a change and gives it its number.
    pub fn record(
        &mut self,
        guid: Guid,
        kind: EditKind,
        before: EditValue,
        after: EditValue,
        note: String,
    ) -> ObjectEdit {
        self.next += 1;
        let edit = ObjectEdit {
            id: self.next,
            guid,
            kind,
            before,
            after,
            note,
        };
        if self.edits.len() >= MAX_EDITS {
            self.edits.remove(0);
        }
        self.edits.push(edit.clone());
        edit
    }

    /// The latest edit, of `guid` when given.
    pub fn last(&self, guid: Option<&Guid>) -> Option<&ObjectEdit> {
        self.edits
            .iter()
            .rev()
            .find(|e| guid.is_none_or(|g| &e.guid == g))
    }

    /// Takes an edit out (after it was undone).
    pub fn take(&mut self, id: u32) -> Option<ObjectEdit> {
        let at = self.edits.iter().position(|e| e.id == id)?;
        Some(self.edits.remove(at))
    }

    /// Drops the edits of `guid` (all when `None`) without undoing them;
    /// how many.
    pub fn forget(&mut self, guid: Option<&Guid>) -> usize {
        let before = self.edits.len();
        self.edits.retain(|e| guid.is_some_and(|g| &e.guid != g));
        before - self.edits.len()
    }

    pub fn list(&self) -> &[ObjectEdit] {
        &self.edits
    }

    /// Whether the link holds the object hidden: its latest
    /// [`EditKind::Shown`] edit hid it.
    pub fn hidden(&self, guid: &Guid) -> bool {
        self.edits
            .iter()
            .rev()
            .find(|e| &e.guid == guid && e.kind == EditKind::Shown)
            .is_some_and(|e| e.after == EditValue::Flag(false))
    }
}

/// The objects the journal moved, each with its matrix after the last
/// move: what "move into the workspace" writes.
pub fn moved(edits: &[ObjectEdit]) -> Vec<(Guid, Matrix)> {
    let mut out: Vec<(Guid, Matrix)> = Vec::new();
    for e in edits {
        if let (EditKind::Transform, EditValue::Matrix(m)) = (e.kind, e.after) {
            match out.iter_mut().find(|(g, _)| g == &e.guid) {
                Some(slot) => slot.1 = m,
                None => out.push((e.guid, m)),
            }
        }
    }
    out
}

/// Turns a matrix by `radians` about the world's up axis (Y), keeping its
/// place, scale and tilt. The same sense as a player's yaw
/// ([`crate::Pose::yaw`]): +Z turns toward +X.
pub fn rotate_y(m: &Matrix, radians: f32) -> Matrix {
    let (s, c) = radians.sin_cos();
    let mut out = *m;
    for row in 0..3 {
        let x = m[row * 4];
        let z = m[row * 4 + 2];
        out[row * 4] = x * c + z * s;
        out[row * 4 + 2] = -x * s + z * c;
    }
    out
}

/// The yaw of a matrix: `atan2(forward.x, forward.z)`.
pub fn yaw(m: &Matrix) -> f32 {
    m[8].atan2(m[10])
}

/// A matrix with a new translation.
pub fn with_position(m: &Matrix, position: [f32; 3]) -> Matrix {
    let mut out = *m;
    out[12..15].copy_from_slice(&position);
    out
}

/// Distance between the translations of two matrices.
pub fn distance(a: &Matrix, b: &Matrix) -> f32 {
    ((a[12] - b[12]).powi(2) + (a[13] - b[13]).powi(2) + (a[14] - b[14]).powi(2)).sqrt()
}

/// A short note on what a transform changed (`moved 1.50 m, turned 90.0°`).
pub fn transform_note(before: &Matrix, after: &Matrix) -> String {
    let d = distance(before, after);
    let mut turn = (yaw(after) - yaw(before)).to_degrees();
    while turn > 180.0 {
        turn -= 360.0;
    }
    while turn <= -180.0 {
        turn += 360.0;
    }
    let mut parts = Vec::new();
    if d >= 0.005 {
        parts.push(format!("moved {d:.2} m"));
    }
    if turn.abs() >= 0.05 {
        parts.push(format!("turned {turn:.1}°"));
    }
    if parts.is_empty() {
        parts.push("matrix set".to_owned());
    }
    parts.join(", ")
}

pub(crate) fn put_matrix(w: &mut Writer, m: &Matrix) {
    for v in m {
        w.f32(*v);
    }
}

pub(crate) fn get_matrix(r: &mut Reader<'_>) -> Result<Matrix, DecodeError> {
    let mut m = [0f32; 16];
    for v in &mut m {
        *v = r.f32()?;
    }
    Ok(m)
}

pub(crate) fn put_state(w: &mut Writer, s: &ObjectState) {
    w.bytes(&s.guid);
    w.u32(s.class);
    w.str(&s.class_name);
    match &s.matrix {
        Some(m) => {
            w.u8(1);
            put_matrix(w, m);
        }
        None => w.u8(0),
    }
    match s.inactive {
        Some(n) => {
            w.u8(1);
            w.u8(n);
        }
        None => w.u8(0),
    }
    w.bool(s.has_model);
    w.bool(s.listening);
    w.bool(s.hidden_by_link);
    w.str(&s.unsupported);
}

pub(crate) fn get_state(r: &mut Reader<'_>) -> Result<ObjectState, DecodeError> {
    Ok(ObjectState {
        guid: r.guid()?,
        class: r.u32()?,
        class_name: r.str()?,
        matrix: if r.bool()? {
            Some(get_matrix(r)?)
        } else {
            None
        },
        inactive: if r.bool()? { Some(r.u8()?) } else { None },
        has_model: r.bool()?,
        listening: r.bool()?,
        hidden_by_link: r.bool()?,
        unsupported: r.str()?,
    })
}

fn put_value(w: &mut Writer, v: &EditValue) {
    match v {
        EditValue::Matrix(m) => {
            w.u8(0);
            put_matrix(w, m);
        }
        EditValue::Flag(f) => {
            w.u8(1);
            w.bool(*f);
        }
    }
}

fn get_value(r: &mut Reader<'_>) -> Result<EditValue, DecodeError> {
    Ok(match r.u8()? {
        0 => EditValue::Matrix(get_matrix(r)?),
        1 => EditValue::Flag(r.bool()?),
        _ => return Err(DecodeError::BadValue("edit value")),
    })
}

pub(crate) fn put_edit(w: &mut Writer, e: &ObjectEdit) {
    w.u32(e.id);
    w.bytes(&e.guid);
    w.u8(e.kind as u8);
    put_value(w, &e.before);
    put_value(w, &e.after);
    w.str(&e.note);
}

pub(crate) fn get_edit(r: &mut Reader<'_>) -> Result<ObjectEdit, DecodeError> {
    Ok(ObjectEdit {
        id: r.u32()?,
        guid: r.guid()?,
        kind: EditKind::from_u8(r.u8()?).ok_or(DecodeError::BadValue("edit kind"))?,
        before: get_value(r)?,
        after: get_value(r)?,
        note: r.str()?,
    })
}

pub(crate) fn put_edits(w: &mut Writer, list: &[ObjectEdit]) {
    w.u32(list.len() as u32);
    for e in list {
        put_edit(w, e);
    }
}

pub(crate) fn get_edits(r: &mut Reader<'_>) -> Result<Vec<ObjectEdit>, DecodeError> {
    let n = r.count(25)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(get_edit(r)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: Matrix = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 2.0, 30.0, 1.0,
    ];

    fn close(a: &Matrix, b: &Matrix) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn turning_about_up_keeps_place_and_scale() {
        let mut scaled = IDENTITY;
        for v in &mut scaled[0..11] {
            *v *= 2.0;
        }
        scaled[15] = 1.0;
        let turned = rotate_y(&scaled, std::f32::consts::FRAC_PI_2);
        // Forward (+Z) now points along +X, right along -Z; lengths stay 2.
        assert!((turned[8] - 2.0).abs() < 1e-5 && turned[10].abs() < 1e-5);
        assert!((turned[2] + 2.0).abs() < 1e-5 && turned[0].abs() < 1e-5);
        assert_eq!(turned[12..16], scaled[12..16]);
        assert!((yaw(&turned) - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!(close(
            &rotate_y(&turned, -std::f32::consts::FRAC_PI_2),
            &scaled
        ));
        assert!(crate::is_world_matrix(&turned, true));
    }

    #[test]
    fn notes_say_what_changed() {
        let moved = with_position(&IDENTITY, [11.5, 2.0, 30.0]);
        assert_eq!(transform_note(&IDENTITY, &moved), "moved 1.50 m");
        let turned = rotate_y(&IDENTITY, (-30f32).to_radians());
        assert_eq!(transform_note(&IDENTITY, &turned), "turned -30.0°");
        assert_eq!(transform_note(&IDENTITY, &IDENTITY), "matrix set");
    }

    #[test]
    fn the_journal_undoes_per_object_and_knows_what_it_hid() {
        let a = [1u8; 16];
        let b = [2u8; 16];
        let mut j = EditJournal::default();
        let moved = with_position(&IDENTITY, [0.0, 0.0, 0.0]);
        let e1 = j.record(
            a,
            EditKind::Transform,
            EditValue::Matrix(IDENTITY),
            EditValue::Matrix(moved),
            "moved".into(),
        );
        let e2 = j.record(
            b,
            EditKind::Shown,
            EditValue::Flag(true),
            EditValue::Flag(false),
            "hidden".into(),
        );
        assert_eq!((e1.id, e2.id), (1, 2));
        assert!(j.hidden(&b) && !j.hidden(&a));
        assert_eq!(j.last(None).unwrap().id, 2);
        assert_eq!(j.last(Some(&a)).unwrap().id, 1);
        assert_eq!(moved_matrices(&j), vec![(a, moved)]);
        let undone = j.take(2).unwrap();
        assert_eq!(undone.inverse().1, EditValue::Flag(true));
        assert!(!j.hidden(&b));
        assert_eq!(j.forget(Some(&b)), 0);
        assert_eq!(j.forget(None), 1);
        assert!(j.list().is_empty());
        // Numbers are not reused.
        let e3 = j.record(
            a,
            EditKind::Listening,
            EditValue::Flag(true),
            EditValue::Flag(false),
            String::new(),
        );
        assert_eq!(e3.id, 3);
    }

    fn moved_matrices(j: &EditJournal) -> Vec<(Guid, Matrix)> {
        moved(j.list())
    }

    #[test]
    fn the_journal_keeps_the_latest() {
        let mut j = EditJournal::default();
        for i in 0..MAX_EDITS + 3 {
            j.record(
                [i as u8; 16],
                EditKind::Shown,
                EditValue::Flag(true),
                EditValue::Flag(false),
                String::new(),
            );
        }
        assert_eq!(j.list().len(), MAX_EDITS);
        assert_eq!(j.list()[0].id, 4);
    }

    #[test]
    fn moves_of_one_object_end_in_its_last_matrix() {
        let g = [7u8; 16];
        let one = with_position(&IDENTITY, [1.0, 0.0, 0.0]);
        let two = with_position(&IDENTITY, [2.0, 0.0, 0.0]);
        let mut j = EditJournal::default();
        for m in [one, two] {
            j.record(
                g,
                EditKind::Transform,
                EditValue::Matrix(IDENTITY),
                EditValue::Matrix(m),
                String::new(),
            );
        }
        assert_eq!(moved(j.list()), vec![(g, two)]);
    }
}
