//! Editing the world while the game runs (stage 47): moving, turning,
//! hiding and showing SimGroup objects, with a journal for undo.
//!
//! An object is changed only through the game's own functions
//! (`doctor_toolkit_live::edit`): its matrix through `Entity::SetMatrix`
//! (vtable `+0x28`), which also moves its model on the screen. A fixed
//! collision does not follow a new matrix by itself (`0x004B6C20` moves only
//! dynamic bodies, and from body to object), so after a move each collision
//! part of the model whose bodies are in the physical world takes them out
//! (`0x004B66A0`) and puts them back (`0x004B6330`, which places every body
//! at the object's matrix) — what the part's own deactivation and
//! activation run. Hiding is `Entity::SetVisible(false)` (`0x00471DB0`, the
//! renderer hides it) and the collision parts' bodies out of the world.
//!
//! Not the whole object out of the world and back (`+0x6C`/`+0x70`): that
//! stops and starts the state machine of its metamodel (`0x004A6FA0`,
//! `0x004A6F30`) and every part, not only the collision.
//!
//! Found on the stand: a `Destructible` whose metamodel has health states
//! (the plane G-STAN of Havana) shows its damaged state after a jump of
//! metres in one frame — with or without the collision placed again; moves
//! of a few centimetres keep it whole. Written into the workspace and
//! loaded again, it stands whole at the new place.
//!
//! The writes run inside a level frame ([`crate::update_hook`]): the
//! renderer gets its commands in the frame's buffer `0x01206880`.

use std::sync::Mutex;

use doctor_toolkit_live::addr::*;
use doctor_toolkit_live::edit::{
    EditJournal, EditKind, EditValue, Matrix, ObjectEdit, ObjectState, transform_note,
};
use doctor_toolkit_live::{Done, ErrorCode, Guid, LoopState, Response};

use crate::game;
use crate::win::Mem;

static JOURNAL: Mutex<EditJournal> = Mutex::new(EditJournal::new());

#[repr(C, align(16))]
struct Aligned(Matrix);

fn journal() -> std::sync::MutexGuard<'static, EditJournal> {
    JOURNAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The object with this GUID, walking the SimManager's table of groups as
/// [`game::world`] does (no game code is called).
fn find(m: &mut Mem, guid: &Guid) -> Option<u32> {
    let sim = m.ptr(SIM_MANAGER)?;
    let buckets = m.ptr(sim + SIM_CLASS_BUCKETS)?;
    let count = m.u32(sim + SIM_CLASS_BUCKET_COUNT)?;
    for i in 0..count.min(1 << 16) {
        let mut record = m.u32(buckets + i * 4).unwrap_or(0);
        let mut records = 0;
        while record >= 0x1_0000 && records < 1024 {
            records += 1;
            let mut object = m.u32(record + 0x10).unwrap_or(0);
            let mut n = 0;
            while object >= 0x1_0000 && n < 100_000 {
                n += 1;
                if game::object_guid(m, object).is_some_and(|g| &g == guid) {
                    return Some(object);
                }
                object = m.u32(object + 8).unwrap_or(0);
            }
            record = m.u32(record + 0x18).unwrap_or(0);
        }
    }
    None
}

/// Why the module does not move or hide this object; `None` when it does.
fn unsupported(m: &mut Mem, object: u32, class_name: &str) -> Option<String> {
    let Some(vtable) = m.u32(object) else {
        return Some("unreadable".into());
    };
    let slot = |m: &mut Mem, s: u32| m.u32(vtable + s).unwrap_or(0);
    let class = if class_name.is_empty() {
        "its class".to_owned()
    } else {
        format!("class {class_name}")
    };
    if slot(m, ENTITY_SET_MATRIX_SLOT) != ENTITY_SET_MATRIX {
        return Some(format!(
            "{class} sets its matrix its own way (not Entity's 0x00470E30): people, cars and the player move by themselves"
        ));
    }
    let matrix = m.f32s::<16>(object + ENTITY_MATRIX).unwrap_or_default();
    if !doctor_toolkit_live::is_world_matrix(&matrix, true) {
        return Some("no world matrix at +0xD0".into());
    }
    if m.u32(object + ENTITY_PARENT).unwrap_or(1) != 0 {
        return Some("attached to another object: its matrix is relative to that one".into());
    }
    None
}

fn class_of(m: &mut Mem, object: u32) -> (u32, String) {
    let registry = game::class_registry(m);
    let known: std::collections::BTreeSet<u32> = registry.iter().map(|(h, _, _)| *h).collect();
    let vtable = m.u32(object).unwrap_or(0);
    let hash = game::class_of_vtable(m, vtable, &known);
    let name = registry
        .iter()
        .find(|(h, _, _)| *h == hash)
        .map(|(_, _, n)| n.clone())
        .unwrap_or_default();
    (hash, name)
}

fn state_of(m: &mut Mem, object: u32, guid: Guid) -> ObjectState {
    let (class, class_name) = class_of(m, object);
    let why = unsupported(m, object, &class_name);
    let entity = why.is_none();
    ObjectState {
        guid,
        class,
        class_name,
        matrix: m
            .f32s::<16>(object + ENTITY_MATRIX)
            .filter(|f| doctor_toolkit_live::is_world_matrix(f, true)),
        inactive: if entity {
            m.u8(object + ENTITY_INACTIVE)
        } else {
            None
        },
        has_model: m.u32(object + ENTITY_MODEL).unwrap_or(0) != 0,
        listening: m.u8(object + OBJECT_LISTENER_FLAGS).unwrap_or(0) & 1 != 0,
        hidden_by_link: journal().hidden(&guid),
        unsupported: why.unwrap_or_default(),
    }
}

fn not_found(guid: &Guid) -> Response {
    Response::error(
        ErrorCode::Failed,
        format!(
            "no object {} in the game (its stream is not loaded, or it was never made)",
            doctor_toolkit_live::guid_hex(guid)
        ),
    )
}

/// Answer to `Object`: the state of one object.
pub fn object(guid: Guid) -> Response {
    let mut m = Mem::mapped();
    match find(&mut m, &guid) {
        Some(o) => Response::Object(state_of(&mut m, o, guid)),
        None => not_found(&guid),
    }
}

/// Where writes may run: a level frame with the renderer's buffer.
fn in_frame(m: &mut Mem) -> Result<(), Box<Response>> {
    if game::loop_state(m) != LoopState::Game {
        return Err(Box::new(Response::error(ErrorCode::NotReady, "not in a level")));
    }
    if m.ptr(FRAME_COMMANDS).is_none() {
        return Err(Box::new(Response::error(
            ErrorCode::NotReady,
            "no frame in progress",
        )));
    }
    Ok(())
}

/// A supported object for a write, or the refusal.
fn target(m: &mut Mem, guid: &Guid) -> Result<(u32, ObjectState), Box<Response>> {
    in_frame(m)?;
    let object = find(m, guid).ok_or_else(|| Box::new(not_found(guid)))?;
    let state = state_of(m, object, *guid);
    if !state.supported() {
        return Err(Box::new(Response::error(
            ErrorCode::Failed,
            format!("not changed: {}", state.unsupported),
        )));
    }
    Ok((object, state))
}

fn method(m: &mut Mem, object: u32, slot: u32) -> Option<u32> {
    let vtable = m.u32(object)?;
    m.u32(vtable + slot)
}

/// A collision part of the object's model: the part, whether the instance
/// activated it, whether its bodies are in the physical world.
struct CollisionPart {
    part: u32,
    active: bool,
    in_world: bool,
}

/// The collision parts of the object's model (`+0x130`, parts `+0x68`).
fn collision_parts(m: &mut Mem, object: u32) -> Vec<CollisionPart> {
    let mut out = Vec::new();
    let Some(instance) = m.ptr(object + ENTITY_MODEL) else {
        return out;
    };
    let (Some(parts), Some(n)) = (
        m.ptr(instance + MODEL_PARTS),
        m.u32(instance + MODEL_PART_COUNT),
    ) else {
        return out;
    };
    for i in 0..n.min(256) {
        let entry = parts + i * MODEL_PART_SIZE;
        let Some(part) = m.ptr(entry) else { continue };
        let flags = m.u32(entry + 4).unwrap_or(0) as u16;
        let marker = m
            .u32(part)
            .filter(|v| (0x00D5_5000..0x00E5_0000).contains(v))
            .and_then(|v| m.u32(v + 8));
        if marker != Some(COLLISION_PART_MARKER) {
            continue;
        }
        out.push(CollisionPart {
            part,
            active: flags & MODEL_PART_ACTIVE != 0,
            in_world: m.u8(part + COLLISION_PART_FLAGS).unwrap_or(0) & 1 != 0,
        });
    }
    out
}

/// Takes the bodies of a collision part out of the physical world, or
/// puts them in at the object's matrix.
fn collision(part: u32, into_world: bool) {
    let f = if into_world {
        COLLISION_ADD
    } else {
        COLLISION_REMOVE
    };
    // SAFETY: the collision part's own functions (checked bytes), `thiscall`
    // without arguments; each checks the part's bit 0 itself.
    unsafe {
        let f: extern "thiscall" fn(u32) = std::mem::transmute(f as usize);
        f(part);
    }
}

/// Shows or hides the object's render instance (`Entity::SetVisible`).
fn set_visible(object: u32, visible: bool) {
    // SAFETY: Entity's function (checked bytes; the object's class keeps
    // Entity's SetMatrix, so its layout), `thiscall`, callee pops the byte;
    // inside a frame, where the command buffer is.
    unsafe {
        let f: extern "thiscall" fn(u32, u32) = std::mem::transmute(ENTITY_SET_VISIBLE as usize);
        f(object, u32::from(visible));
    }
}

/// Gives the object `matrix` and places the bodies of its collision that
/// are in the world again; how many parts.
fn set_matrix(m: &mut Mem, object: u32, matrix: &Matrix) -> Result<usize, String> {
    if method(m, object, ENTITY_SET_MATRIX_SLOT) != Some(ENTITY_SET_MATRIX) {
        return Err("SetMatrix is not Entity's".into());
    }
    let aligned = Aligned(*matrix);
    // SAFETY: Entity::SetMatrix (checked bytes), `thiscall`, callee pops
    // the matrix pointer; the matrix is aligned on 16 for its `movaps`.
    unsafe {
        let f: extern "thiscall" fn(u32, *const Aligned) =
            std::mem::transmute(ENTITY_SET_MATRIX as usize);
        f(object, &aligned);
    }
    let mut placed = 0;
    for p in collision_parts(m, object).iter().filter(|p| p.in_world) {
        collision(p.part, false);
        collision(p.part, true);
        placed += 1;
    }
    Ok(placed)
}

/// Hides the object (render and collision) or shows it again; how many
/// collision parts changed.
fn set_shown(m: &mut Mem, object: u32, shown: bool) -> usize {
    set_visible(object, shown);
    let mut n = 0;
    for p in collision_parts(m, object) {
        if shown && p.active && !p.in_world {
            collision(p.part, true);
            n += 1;
        } else if !shown && p.in_world {
            collision(p.part, false);
            n += 1;
        }
    }
    n
}

fn set_listening(object: u32, on: bool) {
    let at = (object + OBJECT_LISTENER_FLAGS) as usize as *mut u8;
    // SAFETY: the flags byte of the object's listener, read just before;
    // the bus reads it on this same thread.
    unsafe {
        let v = std::ptr::read_volatile(at);
        std::ptr::write_volatile(at, if on { v | 1 } else { v & !1 });
    }
}

/// Answer to `ObjectTransform`.
pub fn transform(guid: Guid, matrix: Matrix) -> Response {
    if !doctor_toolkit_live::is_world_matrix(&matrix, true) {
        return Response::error(
            ErrorCode::BadRequest,
            "not a world matrix (orthogonal rows of length 0.001 to 1000, last column 0, 0, 0, 1, a place within 100 km)",
        );
    }
    let mut m = Mem::mapped();
    let (object, state) = match target(&mut m, &guid) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let Some(before) = state.matrix else {
        return Response::error(ErrorCode::Failed, "no matrix to change");
    };
    let placed = match set_matrix(&mut m, object, &matrix) {
        Ok(n) => n,
        Err(e) => return Response::error(ErrorCode::Failed, e),
    };
    let after = m.f32s::<16>(object + ENTITY_MATRIX).unwrap_or(matrix);
    let mut note = transform_note(&before, &after);
    if placed > 0 {
        note.push_str(&format!(", collision placed again ({placed} part(s))"));
    }
    if state.inactive != Some(0) {
        note.push_str(" (out of the world now: placed when it comes back)");
    }
    Response::ObjectEdit(journal().record(
        guid,
        EditKind::Transform,
        EditValue::Matrix(before),
        EditValue::Matrix(after),
        note,
    ))
}

/// Answer to `ObjectEnable`.
pub fn enable(guid: Guid, enabled: bool, logic: bool) -> Response {
    let mut m = Mem::mapped();
    if logic {
        if let Err(r) = in_frame(&mut m) {
            return *r;
        }
        let Some(object) = find(&mut m, &guid) else {
            return not_found(&guid);
        };
        if m.ptr(object + OBJECT_LISTENER).is_none() {
            return Response::error(ErrorCode::Failed, "the object has no listener");
        }
        let now = m.u8(object + OBJECT_LISTENER_FLAGS).unwrap_or(0) & 1 != 0;
        if now == enabled {
            return Response::error(
                ErrorCode::BadRequest,
                if enabled {
                    "its logic already takes messages"
                } else {
                    "its logic is already off"
                },
            );
        }
        set_listening(object, enabled);
        return Response::ObjectEdit(journal().record(
            guid,
            EditKind::Listening,
            EditValue::Flag(now),
            EditValue::Flag(enabled),
            if enabled {
                "logic on: its listener takes messages".into()
            } else {
                "logic off: its listener takes no messages".into()
            },
        ));
    }
    let (object, state) = match target(&mut m, &guid) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let held = journal().hidden(&guid);
    if enabled && !held {
        return Response::error(
            ErrorCode::BadRequest,
            "the link did not hide it: it shows only what it hid",
        );
    }
    if !enabled && held {
        return Response::error(ErrorCode::BadRequest, "the link hid it already");
    }
    let parts = set_shown(&mut m, object, enabled);
    let mut note = if enabled {
        format!("shown: on the screen, collision back ({parts} part(s))")
    } else {
        format!("hidden: off the screen, collision out of the world ({parts} part(s))")
    };
    if state.inactive != Some(0) {
        note.push_str("; the game keeps it out of the world now");
    }
    Response::ObjectEdit(journal().record(
        guid,
        EditKind::Shown,
        EditValue::Flag(!enabled),
        EditValue::Flag(enabled),
        note,
    ))
}

/// Answer to `ObjectUndo`: applies the value before of the latest edit and
/// takes it out of the journal.
pub fn undo(guid: Option<Guid>) -> Response {
    let Some(edit) = journal().last(guid.as_ref()).cloned() else {
        return Response::error(
            ErrorCode::BadRequest,
            "nothing to undo: the link changed nothing (or it was undone)",
        );
    };
    let mut m = Mem::mapped();
    if let Err(r) = in_frame(&mut m) {
        return *r;
    }
    let Some(object) = find(&mut m, &edit.guid) else {
        return not_found(&edit.guid);
    };
    let done = match (edit.kind, edit.before) {
        (EditKind::Transform, EditValue::Matrix(before)) => {
            if unsupported(&mut m, object, "").is_some() {
                Err("the object can no longer be moved".to_owned())
            } else {
                set_matrix(&mut m, object, &before).map(|_| {
                    let now = m.f32s::<16>(object + ENTITY_MATRIX).unwrap_or(before);
                    format!(
                        "undid #{}: {}",
                        edit.id,
                        transform_note(edit.after.matrix().unwrap_or(&before), &now)
                    )
                })
            }
        }
        (EditKind::Shown, EditValue::Flag(shown)) => {
            set_shown(&mut m, object, shown);
            Ok(format!(
                "undid #{}: {}",
                edit.id,
                if shown { "shown again" } else { "hidden again" }
            ))
        }
        (EditKind::Listening, EditValue::Flag(on)) => {
            set_listening(object, on);
            Ok(format!(
                "undid #{}: logic {}",
                edit.id,
                if on { "on" } else { "off" }
            ))
        }
        _ => Err("a broken edit".to_owned()),
    };
    match done {
        Ok(note) => {
            journal().take(edit.id);
            Response::ObjectEdit(ObjectEdit {
                id: edit.id,
                guid: edit.guid,
                kind: edit.kind,
                before: edit.after,
                after: edit.before,
                note,
            })
        }
        Err(e) => Response::error(ErrorCode::Failed, e),
    }
}

/// Answer to `ObjectEdits`.
pub fn edits() -> Response {
    Response::ObjectEdits(journal().list().to_vec())
}

/// Answer to `ObjectForget`.
pub fn forget(guid: Option<Guid>) -> Response {
    let n = journal().forget(guid.as_ref());
    Response::Done(Done {
        count: n as u32,
        note: format!("forgot {n} edit(s); the game keeps them"),
    })
}
