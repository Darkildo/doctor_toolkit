//! Live debugging (stage 35): breakpoints, pause and step, the free camera
//! and the replacement of a texture ([`doctor_toolkit_live::debug`]).
//!
//! **Stops.** A hit breakpoint holds the game's main thread where it is —
//! in the bus before the message reaches its listeners, in Lua `print`,
//! after a Lua error — or, for a pause, a step and a break hit while the
//! module runs a write of its own, in the next level frame before the
//! game's update. While it stands, [`hold`] answers the requests that came
//! (reads anywhere, writes and Lua only in a frame) and waits for
//! Continue, Step or the hold limit. The game's other threads go on; the
//! main thread holds no lock of ours while it waits.
//!
//! **Free camera.** In the bus hook, when the drawing of a frame sends
//! `iMsgBeforeRender` (`0x0040E1F0`), the module sets the world matrix of
//! the view about to be drawn with the game's own setter (`0x00422D40`):
//! the inverse, the planes and the render thread's copy follow.
//!
//! **Texture.** In a level frame, on the main thread: the texture is found
//! by name hash in the game's map (`0x00611000` without calling it), its
//! entry checked (made, 2D, same size, format and levels, not being
//! loaded), and the loader's own command "fill the levels" goes into the
//! frame's command buffer — the render thread fills it on the next frame.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use doctor_toolkit_live::addr::*;
use doctor_toolkit_live::debug::{
    BreakRule, Control, DebugState, HOLD_DEFAULT_S, HOLD_MAX_S, MAX_RULES, Stop, StopPlace,
    StopReason, texture_bytes,
};
use doctor_toolkit_live::{CameraView, Done, ErrorCode, Guid, LoopState, Response};

use crate::win::{self, Mem};

struct Debug {
    rules: Vec<BreakRule>,
    hold_limit_s: u16,
    hits: u32,
    /// The stop and when it began.
    stop: Option<(Stop, Instant)>,
    /// A pause, a step or a break to stop at in the next level frame.
    pending: Option<StopReason>,
    camera: Option<CameraView>,
}

static DEBUG: Mutex<Debug> = Mutex::new(Debug {
    rules: Vec::new(),
    hold_limit_s: HOLD_DEFAULT_S,
    hits: 0,
    stop: None,
    pending: None,
    camera: None,
});
/// Fast checks for the hooks.
static HAS_RULES: AtomicBool = AtomicBool::new(false);
static HAS_PENDING: AtomicBool = AtomicBool::new(false);
static HAS_CAMERA: AtomicBool = AtomicBool::new(false);
/// The main thread stands in [`hold`]: hooks do not stop it again.
static HOLDING: AtomicBool = AtomicBool::new(false);
/// What the client said while it stood: 1 go on, 2 one frame.
static RESUME: AtomicU8 = AtomicU8::new(0);
/// Id of the game's main thread (the frame hook records it).
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
/// The main thread runs a request of the link (a message the link sends
/// must not hold the reply it waits for: the stop moves to the next frame).
static IN_JOB: AtomicBool = AtomicBool::new(false);

fn state() -> std::sync::MutexGuard<'static, Debug> {
    DEBUG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called by the frame hook: this is the main thread.
pub fn mark_main_thread() {
    // SAFETY: a plain Win32 call.
    MAIN_THREAD.store(unsafe { win::GetCurrentThreadId() }, Ordering::Relaxed);
}

fn on_main_thread() -> bool {
    // SAFETY: as above.
    let me = unsafe { win::GetCurrentThreadId() };
    me != 0 && me == MAIN_THREAD.load(Ordering::Relaxed)
}

/// Runs a request of the link on the main thread.
pub fn as_job<T>(f: impl FnOnce() -> T) -> T {
    let was = IN_JOB.swap(true, Ordering::Relaxed);
    let out = f();
    IN_JOB.store(was, Ordering::Relaxed);
    out
}

// --- Requests (network threads) ----------------------------------------------

pub fn set_rules(mut rules: Vec<BreakRule>, hold_s: u16) -> Response {
    rules.truncate(MAX_RULES);
    let n = rules.len();
    let mut s = state();
    s.rules = rules;
    s.hold_limit_s = if hold_s == 0 {
        HOLD_DEFAULT_S
    } else {
        hold_s.min(HOLD_MAX_S)
    };
    HAS_RULES.store(n > 0, Ordering::Relaxed);
    let limit = s.hold_limit_s;
    drop(s);
    crate::log(&format!(
        "{n} breakpoint(s), a stop lasts at most {limit} s"
    ));
    Response::Done(Done {
        count: n as u32,
        note: format!("{n} breakpoint(s)"),
    })
}

pub fn control(c: Control) -> Response {
    let mut s = state();
    let note = match c {
        Control::Pause => {
            if s.stop.is_some() {
                "already stopped".to_owned()
            } else {
                s.pending = Some(StopReason::Pause);
                HAS_PENDING.store(true, Ordering::Relaxed);
                "pauses in the next level frame".to_owned()
            }
        }
        Control::Continue => {
            s.pending = None;
            HAS_PENDING.store(false, Ordering::Relaxed);
            if s.stop.is_some() {
                RESUME.store(1, Ordering::Relaxed);
                "goes on".to_owned()
            } else {
                "was running".to_owned()
            }
        }
        Control::Step => {
            if s.stop.is_some() {
                RESUME.store(2, Ordering::Relaxed);
            } else {
                s.pending = Some(StopReason::Step);
                HAS_PENDING.store(true, Ordering::Relaxed);
            }
            "one frame".to_owned()
        }
    };
    Response::Done(Done { count: 0, note })
}

pub fn debug_state() -> DebugState {
    let s = state();
    DebugState {
        rules: s.rules.clone(),
        stop: s.stop.as_ref().map(|(stop, since)| {
            let held = since.elapsed();
            let limit = Duration::from_secs(u64::from(s.hold_limit_s));
            Stop {
                held_ms: held.as_millis().min(u128::from(u32::MAX)) as u32,
                resume_in_ms: limit.saturating_sub(held).as_millis() as u32,
                ..stop.clone()
            }
        }),
        pending: s.pending.is_some(),
        hits: s.hits,
        hold_limit_s: s.hold_limit_s,
        camera: s.camera,
    }
}

pub fn set_camera(camera: Option<CameraView>) -> Response {
    if let Some(c) = &camera {
        let p = &c.pose;
        let finite = [p.position, p.right, p.up, p.forward]
            .iter()
            .flatten()
            .all(|v| v.is_finite());
        if !finite {
            return Response::error(ErrorCode::BadRequest, "the camera is not finite");
        }
    }
    let on = camera.is_some();
    state().camera = camera;
    HAS_CAMERA.store(on, Ordering::Relaxed);
    crate::log(if on {
        "free camera on: the game draws with the camera of the link"
    } else {
        "free camera off"
    });
    Response::Done(Done {
        count: u32::from(on),
        note: if on {
            "the game draws with this camera".into()
        } else {
            "the game's camera again".into()
        },
    })
}

// --- Hooks (game threads) ----------------------------------------------------------

/// A breakpoint was hit: stop here, or in the next frame when this is a
/// request of the link or not the main thread.
fn hit(reason: StopReason, place: StopPlace, once: Option<usize>) {
    {
        let mut s = state();
        s.hits += 1;
        if let Some(i) = once
            && i < s.rules.len()
        {
            s.rules.remove(i);
            HAS_RULES.store(!s.rules.is_empty(), Ordering::Relaxed);
        }
        if IN_JOB.load(Ordering::Relaxed) || !on_main_thread() {
            s.pending = Some(reason);
            HAS_PENDING.store(true, Ordering::Relaxed);
            return;
        }
    }
    hold(reason, place);
}

/// The first rule that matches, and whether it is a one-shot.
fn first_hit(test: impl Fn(&BreakRule) -> bool) -> Option<Option<usize>> {
    let s = state();
    s.rules
        .iter()
        .position(test)
        .map(|i| s.rules[i].once.then_some(i))
}

/// Before the bus delivers a message (its thunk, any thread).
pub fn before_message(hash: u32, data: u32, forced: bool) {
    if HAS_CAMERA.load(Ordering::Relaxed) && hash != 0 && on_main_thread() {
        // SAFETY: a variable of the image.
        let before_render =
            unsafe { std::ptr::read_volatile(BEFORE_RENDER_HASH as usize as *const u32) };
        if hash == before_render {
            apply_camera();
        }
    }
    if !HAS_RULES.load(Ordering::Relaxed) || HOLDING.load(Ordering::Relaxed) {
        return;
    }
    let mut receivers: Option<Vec<Guid>> = None;
    let mut get = || {
        receivers
            .get_or_insert_with(|| {
                // SAFETY: the table the bus is about to walk.
                let listeners = unsafe { crate::game::bus_listeners(hash, !forced) };
                let mut m = Mem::default();
                listeners
                    .iter()
                    .filter_map(|&l| crate::game::object_guid(&mut m, l - OBJECT_LISTENER))
                    .filter(doctor_toolkit_live::is_object_guid)
                    .collect()
            })
            .clone()
    };
    let rules = state().rules.clone();
    let Some(i) = rules.iter().position(|r| r.hit_by_message(hash, &mut get)) else {
        return;
    };
    let mut list = get();
    list.truncate(doctor_toolkit_live::journal::MAX_RECEIVERS);
    hit(
        StopReason::Message {
            hash,
            data,
            receivers: list,
        },
        StopPlace::Bus,
        rules[i].once.then_some(i),
    );
}

/// After the game printed `text` (Lua `print` or `OutputDebugStringA`).
pub fn after_output(text: &str, script: &str, line: i32) {
    if !HAS_RULES.load(Ordering::Relaxed) || HOLDING.load(Ordering::Relaxed) {
        return;
    }
    if let Some(once) = first_hit(|r| r.hit_by_output(text)) {
        hit(
            StopReason::Output {
                text: doctor_toolkit_live::journal::clip(text),
                script: script.to_owned(),
                line,
            },
            StopPlace::Print,
            once,
        );
    }
}

/// After a Lua error.
pub fn after_lua_error(message: &str) {
    if !HAS_RULES.load(Ordering::Relaxed) || HOLDING.load(Ordering::Relaxed) {
        return;
    }
    if let Some(once) = first_hit(|r| matches!(r.on, doctor_toolkit_live::debug::BreakOn::LuaError))
    {
        hit(
            StopReason::LuaError {
                message: doctor_toolkit_live::journal::clip(message),
            },
            StopPlace::Lua,
            once,
        );
    }
}

/// In a level frame, before the module's writes and the game's update: a
/// pause or step asked for, or a break moved here.
pub fn frame_point() {
    if !HAS_PENDING.load(Ordering::Relaxed) || HOLDING.load(Ordering::Relaxed) {
        return;
    }
    let reason = {
        let mut s = state();
        HAS_PENDING.store(false, Ordering::Relaxed);
        s.pending.take()
    };
    if let Some(reason) = reason {
        hold(reason, StopPlace::Frame);
    }
}

/// Holds the main thread until the client says go on or the limit passes,
/// answering requests meanwhile.
fn hold(reason: StopReason, place: StopPlace) {
    if HOLDING.swap(true, Ordering::Relaxed) {
        return;
    }
    RESUME.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let mut m = Mem::default();
    let sim = if crate::loop_state() == LoopState::Game {
        m.u32(SIM_TIME_MS).unwrap_or(0)
    } else {
        0
    };
    let stop = Stop {
        reason: reason.clone(),
        place,
        frame: crate::frames(),
        sim_time_ms: sim,
        seq: crate::journal::next_seq(),
        held_ms: 0,
        resume_in_ms: 0,
    };
    let limit = {
        let mut s = state();
        s.stop = Some((stop, started));
        Duration::from_secs(u64::from(s.hold_limit_s))
    };
    crate::log(&format!("stopped in the {}: {reason:?}", place.name()));
    let why = loop {
        match RESUME.swap(0, Ordering::Relaxed) {
            1 => break "continue",
            2 => {
                let mut s = state();
                s.pending = Some(StopReason::Step);
                HAS_PENDING.store(true, Ordering::Relaxed);
                break "step";
            }
            _ => {}
        }
        if started.elapsed() >= limit {
            break "the hold limit passed";
        }
        let _ = std::panic::catch_unwind(|| crate::serve_held(place.allows_writes()));
        std::thread::sleep(Duration::from_millis(5));
    };
    state().stop = None;
    HOLDING.store(false, Ordering::Relaxed);
    crate::log(&format!(
        "goes on after {:.1} s ({why})",
        started.elapsed().as_secs_f32()
    ));
}

/// A 4×4 matrix aligned for the game's `MOVAPS`.
#[repr(C, align(16))]
struct Matrix([f32; 16]);

/// Sets the view about to be drawn to the free camera.
fn apply_camera() {
    let Some(c) = state().camera else { return };
    let mut m = Mem::default();
    let Some(view) = crate::game::current_view(&mut m) else {
        return;
    };
    // Only over a view that holds a world matrix (as the game reads it).
    let Some(now) = m.f32s::<16>(view + CAMERA_MATRIX) else {
        return;
    };
    if !doctor_toolkit_live::is_world_matrix(&now, false) {
        return;
    }
    let p = c.pose;
    let matrix = Matrix([
        p.right[0],
        p.right[1],
        p.right[2],
        0.0,
        p.up[0],
        p.up[1],
        p.up[2],
        0.0,
        p.forward[0],
        p.forward[1],
        p.forward[2],
        0.0,
        p.position[0],
        p.position[1],
        p.position[2],
        1.0,
    ]);
    if !doctor_toolkit_live::is_world_matrix(&matrix.0, false) {
        return;
    }
    // SAFETY: the game's setter of a view (checked bytes): EAX the view,
    // ESI the aligned matrix, `bool invert` on the stack, popped by the
    // callee; on the main thread during the drawing of a frame, where the
    // game itself sets views.
    unsafe {
        std::arch::asm!(
            "push esi",
            "mov esi, edi",
            "push 1",
            "mov edx, {f}",
            "call edx",
            "pop esi",
            f = const VIEW_SET_MATRIX,
            inout("eax") view => _,
            in("edi") matrix.0.as_ptr(),
            clobber_abi("C"),
        );
    }
}

// --- Texture ------------------------------------------------------------------------

/// Data handed to the render thread, kept until it surely ran the command
/// (frames of the main loop when it was given).
static KEPT: Mutex<Vec<(u32, Box<[u8]>)>> = Mutex::new(Vec::new());
/// Frames a texture's data is kept after its command.
const KEEP_FRAMES: u32 = 600;

/// What a texture entry of the game holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureEntry {
    pub handle: u32,
    pub state: u8,
    pub levels: u8,
    pub width: u16,
    pub height: u16,
    pub format: u32,
    pub flags: u32,
}

/// The handle of the texture named `name` (`0x00611000`, read not called).
fn texture_handle(m: &mut Mem, name: u32) -> Option<u32> {
    if name == 0 {
        return None;
    }
    let map = m.ptr(TEXTURE_MAP)?;
    let mut node = m.u32(map + (name & 0xFFF) * 4)?;
    let mut n = 0;
    while node >= 0x1_0000 && n < 100_000 {
        n += 1;
        if m.u32(node + TEXTURE_NODE_NAME)? == name {
            return m.u32(node + TEXTURE_NODE_HANDLE);
        }
        node = m.u32(node + TEXTURE_NODE_NEXT)?;
    }
    None
}

fn texture_entry(m: &mut Mem, handle: u32) -> Option<TextureEntry> {
    if handle >= 0x1000 {
        return None;
    }
    let e = TEXTURE_TABLE + handle * TEXTURE_ENTRY;
    let mut b = [0u8; 16];
    if !m.bytes(e, &mut b) {
        return None;
    }
    Some(TextureEntry {
        handle,
        state: b[0],
        levels: b[1],
        width: u16::from_le_bytes([b[2], b[3]]),
        height: u16::from_le_bytes([b[4], b[5]]),
        format: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        flags: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
    })
}

/// Whether the game's texture loader has an upload of `handle` in flight.
fn texture_loading(m: &mut Mem, handle: u32) -> bool {
    let Some(loader) = m.ptr(TEXTURE_LOADER) else {
        return true;
    };
    let (Some(list), Some(n)) = (
        m.u32(loader + TEXTURE_LOADER_PENDING),
        m.u32(loader + TEXTURE_LOADER_PENDING_COUNT),
    ) else {
        return true;
    };
    if n > 10_000 {
        return true;
    }
    (0..n).any(|i| m.u32(list + i * 0x14 + 4) == Some(handle))
}

/// New levels for a loaded texture (main thread, inside a level frame).
pub fn texture(name: u32, data: &[u8]) -> Response {
    let mut m = Mem::default();
    if crate::loop_state() != LoopState::Game {
        return Response::error(ErrorCode::NotReady, "not in a level");
    }
    let Some(handle) = texture_handle(&mut m, name) else {
        return Response::error(
            ErrorCode::Failed,
            format!("no loaded texture is named {name:#010x} (its stream is not loaded)"),
        );
    };
    let Some(e) = texture_entry(&mut m, handle) else {
        return Response::error(ErrorCode::Failed, format!("bad texture handle {handle}"));
    };
    if !(2..=3).contains(&e.state) {
        return Response::error(
            ErrorCode::NotReady,
            format!("texture {handle} is not ready (state {})", e.state),
        );
    }
    if e.flags & 0xE != 0 {
        return Response::error(
            ErrorCode::Unsupported,
            "a cube, volume or render target texture",
        );
    }
    let Some(need) = texture_bytes(
        e.format,
        u32::from(e.width),
        u32::from(e.height),
        u32::from(e.levels),
    ) else {
        return Response::error(
            ErrorCode::Unsupported,
            format!("texture format {} is not one the module fills", e.format),
        );
    };
    if (data.len() as u64) < need {
        return Response::error(
            ErrorCode::Failed,
            format!(
                "{} bytes, the texture needs {need} ({}×{}, {} levels, format {})",
                data.len(),
                e.width,
                e.height,
                e.levels,
                e.format
            ),
        );
    }
    if texture_loading(&mut m, handle) {
        return Response::error(
            ErrorCode::NotReady,
            "the game is loading this texture now; try again",
        );
    }
    let Some(buffer) = m.ptr(FRAME_COMMANDS) else {
        return Response::error(ErrorCode::NotReady, "no frame command buffer");
    };
    let Some(cursor) = m
        .u32(buffer + 0x14)
        .filter(|c| c % 4 == 0 && *c >= 0x1_0000)
    else {
        return Response::error(ErrorCode::NotReady, "no frame command buffer");
    };
    // The data: the game reads `need` bytes level by level; a few spare
    // bytes past them in case a level is rounded up.
    let mut copy = Vec::with_capacity(data.len() + 4096);
    copy.extend_from_slice(data);
    copy.resize(data.len() + 4096, 0);
    let copy = copy.into_boxed_slice();
    let ptr = copy.as_ptr() as usize as u32;
    // SAFETY: the frame's command buffer on the main thread inside the
    // frame, written like the texture loader writes it (`0x00619EF0`):
    // command, handle, size, data; then the cursor moves on.
    unsafe {
        let at = cursor as usize as *mut u32;
        std::ptr::write_volatile(at, TEXTURE_FILL_COMMAND);
        std::ptr::write_volatile(at.add(1), handle);
        std::ptr::write_volatile(at.add(2), need as u32);
        std::ptr::write_volatile(at.add(3), ptr);
        std::ptr::write_volatile((buffer + 0x14) as usize as *mut u32, cursor + 16);
    }
    let now = crate::frames();
    if let Ok(mut kept) = KEPT.lock() {
        kept.retain(|(f, _)| now.wrapping_sub(*f) < KEEP_FRAMES);
        kept.push((now, copy));
    }
    crate::log(&format!(
        "texture {name:#010x} (handle {handle}, {}×{}, {} levels, format {}): {need} bytes to the render thread",
        e.width, e.height, e.levels, e.format
    ));
    Response::Done(Done {
        count: 1,
        note: format!(
            "texture {}×{}, {} levels: {need} bytes, on screen in the next frame",
            e.width, e.height, e.levels
        ),
    })
}
