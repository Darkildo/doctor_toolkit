//! The game's journal (stage 49): hooks that record what the game prints,
//! the errors of its Lua code and the messages of its bus into the ring of
//! [`doctor_toolkit_live::journal`], which clients read with `Events`.
//!
//! - `OutputDebugStringA`: the import slot `0x00D550C4` points to
//!   [`output_hook`], which records the text and calls the function that was
//!   there;
//! - Lua `print`: the game's print calls the function in `0x0110A6C0`; the
//!   release build has an empty one there (`0x0046BFC0`), the module puts
//!   [`print_hook`] in its place;
//! - Lua errors: the calls of `luaD_rawrunprotected` in `lua_pcall`, the
//!   parser and `lua_resume` go to [`protected_hook`], which reads the
//!   error and the Lua call stack when the call failed;
//! - messages: the first five bytes of the bus `0x00408A00` jump to a thunk
//!   that records the message (while a client asks for messages) and then
//!   runs those five bytes and goes on in the bus;
//! - who sends (stage 78): the three places where the game calls a
//!   listener's handler (`0x00408A8B` in the broadcast, `0x00408C31` in the
//!   delivery to one listener, `0x00440819` in the delayed queue) call a
//!   thunk that leaves a mark on the stack — the listener — for as long as
//!   the handler runs. A message sent meanwhile finds the innermost mark:
//!   that listener's object is its sender. The same thunks record the
//!   deliveries to one listener, which never reach `0x00408A00`.
//!
//! Every hook catches panics, takes the ring's lock only for a moment and
//! never blocks the game for long.

use std::arch::global_asm;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use doctor_toolkit_live::LoopState;
use doctor_toolkit_live::addr::*;
use doctor_toolkit_live::journal::{
    Counts, EventBatch, EventBody, MAX_RECEIVERS, MAX_TRACEBACK, MessageFrom, MessageOrigin,
    OutputSource, Ring, clip, mask,
};

use crate::win::{self, Mem};

static RING: Mutex<Option<Ring>> = Mutex::new(None);
static STARTED: OnceLock<Instant> = OnceLock::new();
/// Kinds the hooks record ([`mask`]): those that could be installed.
static RECORDING: AtomicU8 = AtomicU8::new(0);
/// Until when (milliseconds since start) messages are recorded: a client
/// that asks for them keeps this ahead.
static MESSAGES_UNTIL: AtomicU32 = AtomicU32::new(0);
/// Messages over the rate limit of their hash, counted for the next
/// [`EventBody::MessageCounts`], and when the last one was recorded.
static COUNTS: Mutex<Option<(Counts, u32)>> = Mutex::new(None);
/// The live link itself sends a message now (the bus records it as ours).
static LINK_SENDING: AtomicBool = AtomicBool::new(false);
/// What the import slot of `OutputDebugStringA` held.
static OUTPUT_ORIGINAL: AtomicU32 = AtomicU32::new(0);
/// What the Lua print callback held (`0x0046BFC0`, an empty function).
static PRINT_ORIGINAL: AtomicU32 = AtomicU32::new(0);

/// How long a request for messages keeps their recording on.
const MESSAGES_FOR_MS: u32 = 15_000;

fn now_ms() -> u32 {
    STARTED.get_or_init(Instant::now).elapsed().as_millis() as u32
}

fn second() -> u64 {
    u64::from(now_ms() / 1000)
}

fn ring() -> std::sync::MutexGuard<'static, Option<Ring>> {
    RING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Game time in milliseconds, 0 outside a level (the state of the last
/// pass of the main loop; the time is a variable of the image).
fn sim_time() -> u32 {
    if crate::loop_state() == LoopState::Game {
        // SAFETY: a 4-byte variable in the data of the image (always mapped).
        unsafe { std::ptr::read_volatile(SIM_TIME_MS as usize as *const u32) }
    } else {
        0
    }
}

/// Whether an event of `kind` would pass the rate limit now; if not, it is
/// counted as lost.
fn admit(kind: u8) -> bool {
    let mut r = ring();
    let Some(r) = r.as_mut() else {
        return false;
    };
    let s = second();
    if r.admits(s, kind) {
        true
    } else {
        r.lose(s, kind);
        false
    }
}

/// Records an event.
pub fn record(body: EventBody) {
    let frame = crate::frames();
    let sim = sim_time();
    if let Some(r) = ring().as_mut() {
        r.push(second(), frame, sim, body);
    }
}

/// A note of the module in the journal (what it also writes to its log).
pub fn note(text: &str) {
    if RING.try_lock().is_ok_and(|r| r.is_some()) {
        record(EventBody::Output {
            source: OutputSource::Module,
            text: clip(text),
            script: String::new(),
            line: 0,
        });
    }
}

/// Events for a client. Asking for messages keeps their recording on.
pub fn events(after: u64, kinds: u8, max: u16) -> EventBatch {
    if kinds & mask::MESSAGE != 0 {
        MESSAGES_UNTIL.store(now_ms().saturating_add(MESSAGES_FOR_MS), Ordering::Relaxed);
    }
    let recording = RECORDING.load(Ordering::Relaxed);
    match ring().as_ref() {
        Some(r) => {
            let mut b = r.after(after, kinds, usize::from(max.clamp(1, 2000)));
            b.recording = recording;
            b
        }
        None => EventBatch::default(),
    }
}

/// Number the next event will get.
pub fn next_seq() -> u64 {
    ring().as_ref().map_or(0, Ring::next_seq)
}

/// Once per pass of the main loop: about once a second, the counts of
/// messages over their rate limit become an event.
pub fn tick() {
    let now = now_ms();
    let Ok(mut c) = COUNTS.try_lock() else { return };
    let (counts, last) = c.get_or_insert_with(|| (Counts::default(), now));
    let window = now.wrapping_sub(*last);
    if window < 1000 {
        return;
    }
    *last = now;
    if let Some(body) = counts.take(window) {
        drop(c);
        record(body);
    }
}

/// Marks the messages sent while `f` runs as sent by the live link.
pub fn as_link<T>(f: impl FnOnce() -> T) -> T {
    LINK_SENDING.store(true, Ordering::Relaxed);
    let out = f();
    LINK_SENDING.store(false, Ordering::Relaxed);
    out
}

/// A C string of the game, at most [`doctor_toolkit_live::journal::MAX_TEXT`]
/// bytes.
///
/// # Safety
/// `p` is null or a string the game is about to read itself.
unsafe fn c_text(p: *const u8) -> String {
    if p.is_null() || (p as usize) < 0x1_0000 {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the caller's string ends with a NUL; at most MAX_TEXT bytes.
    while len < doctor_toolkit_live::journal::MAX_TEXT && unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` bytes were just read.
    let bytes = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

// --- What the game prints ------------------------------------------------------

/// Stands in the import slot of `OutputDebugStringA`.
extern "system" fn output_hook(text: *const u8) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: the game passes a C string to OutputDebugStringA.
        let t = unsafe { c_text(text) };
        let t = t.trim_end_matches(['\r', '\n']);
        if !t.trim().is_empty() && admit(mask::OUTPUT) {
            record(EventBody::Output {
                source: OutputSource::Debug,
                text: clip(t),
                script: String::new(),
                line: 0,
            });
        }
        crate::debug::after_output(t, "", 0);
    });
    let original = OUTPUT_ORIGINAL.load(Ordering::Relaxed);
    if original != 0 {
        // SAFETY: what the import slot held: OutputDebugStringA of kernel32
        // (or the hook of another mod, which takes the same argument).
        let f: extern "system" fn(*const u8) = unsafe { std::mem::transmute(original as usize) };
        f(text);
    }
}

/// The game's Lua `print` calls this with the script and line of its
/// caller and the printed text (`0x0046BF77`, `cdecl`).
extern "C" fn print_hook(script: *const u8, line: i32, what: *const u8, text: *const u8) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: strings of the Lua state and of the game's print.
        let (s, t) = unsafe { (c_text(script), c_text(text)) };
        let script = if s.starts_with(['@', '=']) {
            short_source(&s)
        } else {
            s
        };
        if admit(mask::OUTPUT) {
            record(EventBody::Output {
                source: OutputSource::Print,
                text: clip(&t),
                script: script.clone(),
                line,
            });
        }
        crate::debug::after_output(&t, &script, line);
    });
    let original = PRINT_ORIGINAL.load(Ordering::Relaxed);
    if original != 0 {
        // SAFETY: a callback someone else set before, with this signature.
        let f: extern "C" fn(*const u8, i32, *const u8, *const u8) =
            unsafe { std::mem::transmute(original as usize) };
        f(script, line, what, text);
    }
}

// --- Lua errors -------------------------------------------------------------------

/// Text of a Lua string (`TString*`).
fn lua_string(m: &mut Mem, ts: u32) -> Option<String> {
    let len = m.u32(ts + LUA_STRING_LEN)?.min(4096) as usize;
    let mut buf = vec![0u8; len];
    m.bytes(ts + LUA_STRING_TEXT, &mut buf)
        .then(|| String::from_utf8_lossy(&buf).into_owned())
}

/// A chunk name as Lua shows it: `@file` → `file`, `=name` → `name`, the
/// code of a string chunk → `[string "…"]`.
pub fn short_source(source: &str) -> String {
    if let Some(s) = source
        .strip_prefix('@')
        .or_else(|| source.strip_prefix('='))
    {
        s.to_owned()
    } else {
        let first = source.lines().next().unwrap_or_default();
        let mut cut: String = first.chars().take(40).collect();
        if cut.len() < source.len() {
            cut.push('…');
        }
        format!("[string \"{cut}\"]")
    }
}

/// One level of the Lua call stack: chunk, current line, line where the
/// function starts (`None` for a C function).
fn lua_level(m: &mut Mem, ci: u32) -> Option<(String, i32, i32)> {
    let base = m.u32(ci)?;
    let state = m.u32(ci + LUA_CI_STATE)?;
    // The function is the slot below the base: type 6, closure in the value.
    if m.u32(base.checked_sub(8)?)? != 6 {
        return None;
    }
    let closure = m.ptr(base - 4)?;
    if state & 1 != 0 || m.u8(closure + LUA_CLOSURE_IS_C)? != 0 {
        return Some((String::new(), -1, -1));
    }
    let proto = m.ptr(closure + LUA_CLOSURE_PROTO)?;
    let source = m
        .ptr(proto + LUA_PROTO_SOURCE)
        .and_then(|s| lua_string(m, s))
        .unwrap_or_else(|| "?".to_owned());
    let defined = m.u32(proto + LUA_PROTO_LINEDEFINED).unwrap_or(0) as i32;
    // The current instruction: saved in the CallInfo, or a local of the
    // interpreter that ran it (bit 1); that frame is gone after the error,
    // so the value is only trusted when it points into the code.
    let pc = if state & 2 != 0 {
        m.ptr(ci + LUA_CI_PC).and_then(|p| m.u32(p))
    } else {
        m.u32(ci + LUA_CI_SAVEDPC)
    };
    let code = m.u32(proto + LUA_PROTO_CODE).unwrap_or(0);
    let size = m.u32(proto + LUA_PROTO_SIZELINEINFO).unwrap_or(0);
    let line = pc
        .filter(|&pc| pc > code && (pc - code).is_multiple_of(4))
        .map(|pc| (pc - code) / 4 - 1)
        .filter(|&i| i < size.min(1 << 20))
        .and_then(|i| {
            let lines = m.ptr(proto + LUA_PROTO_LINEINFO)?;
            m.u32(lines + i * 4)
        })
        .map_or(-1, |l| l as i32);
    Some((short_source(&source), line, defined))
}

/// The Lua call stack of `L`: `L->ci` down to `L->base_ci`, innermost
/// first, and the chunk and line of the innermost Lua function.
fn lua_stack(m: &mut Mem, l: u32) -> (Vec<String>, String, i32) {
    let mut traceback = Vec::new();
    let mut chunk = String::new();
    let mut line = -1;
    let (Some(mut ci), Some(base_ci)) = (m.u32(l + LUA_CI), m.u32(l + LUA_BASE_CI)) else {
        return (traceback, chunk, line);
    };
    let mut levels = 0;
    while ci > base_ci && levels < 64 && traceback.len() < MAX_TRACEBACK {
        levels += 1;
        let mut text = match lua_level(m, ci) {
            Some((src, _, _)) if src.is_empty() => "[C]: ?".to_owned(),
            Some((src, l, defined)) => {
                if chunk.is_empty() {
                    chunk = src.clone();
                    line = l;
                }
                let at = if l >= 0 {
                    format!("{src}:{l}")
                } else {
                    src.clone()
                };
                if defined == 0 {
                    format!("{at}: in main chunk")
                } else {
                    format!("{at}: in function <{src}:{defined}>")
                }
            }
            None => "?".to_owned(),
        };
        // Lua 5.0 reuses the CallInfo of a tail call.
        let tail = m.u32(ci + LUA_CI_TAILCALLS).unwrap_or(0);
        if tail > 0 && tail < 1 << 20 && m.u32(ci + LUA_CI_STATE).unwrap_or(1) & 1 == 0 {
            text += &format!(" (after {tail} tail call(s))");
        }
        traceback.push(text);
        ci -= LUA_CI_SIZE;
    }
    (traceback, chunk, line)
}

/// The stack of the last run time error, read by `luaG_errormsg` while the
/// interpreter that raised it still runs (its `pc` is alive): Lua state,
/// traceback, chunk, line.
type Captured = (u32, Vec<String>, String, i32);
static CAPTURED: Mutex<Option<Captured>> = Mutex::new(None);

/// Called by the thunk at the start of `luaG_errormsg` (`0x006338A0`, `L`
/// in `ESI`): every run time error passes here before Lua unwinds.
extern "C" fn on_error(l: u32) {
    let _ = std::panic::catch_unwind(|| {
        let mut m = Mem::default();
        let (traceback, chunk, line) = lua_stack(&mut m, l);
        if let Ok(mut c) = CAPTURED.lock() {
            *c = Some((l, traceback, chunk, line));
        }
    });
}

/// The line of `chunk:LINE: …` at the start of an error message.
fn line_in_message(message: &str, chunk: &str) -> Option<i32> {
    message
        .strip_prefix(chunk)?
        .strip_prefix(':')?
        .split(':')
        .next()?
        .parse()
        .ok()
}

/// The error of a failed protected call: its message on top of the stack,
/// and the call stack — captured by `luaG_errormsg` for run time errors, or
/// as `L->ci` still is (syntax and memory errors).
fn lua_error(l: u32, status: i32) -> Option<EventBody> {
    let mut m = Mem::default();
    let top = m.u32(l + LUA_TOP)?;
    let message = match status {
        // Run time and syntax errors leave their message on top.
        1 | 3 => {
            let at = top.checked_sub(8)?;
            match m.u32(at)? {
                4 => m.ptr(at + 4).and_then(|s| lua_string(&mut m, s)),
                0 => Some("nil".to_owned()),
                t => Some(format!("(error object of type {t})")),
            }
            .unwrap_or_default()
        }
        4 => "not enough memory".to_owned(),
        5 => "error in error handling".to_owned(),
        s => format!("Lua error {s}"),
    };
    let captured = CAPTURED
        .lock()
        .ok()
        .and_then(|mut c| c.take())
        .filter(|c| c.0 == l && status == 1);
    let (traceback, chunk, mut line) = match captured {
        Some((_, t, c, l)) => (t, c, l),
        None => lua_stack(&mut m, l),
    };
    if line < 0
        && let Some(n) = line_in_message(&message, &chunk)
    {
        line = n;
    }
    Some(EventBody::LuaError {
        status: status as u8,
        message: clip(&message),
        chunk,
        line,
        traceback,
    })
}

/// Stands for `luaD_rawrunprotected` in the calls the module hooks (same
/// arguments, `cdecl`).
extern "C" fn protected_hook(l: u32, f: u32, ud: u32) -> i32 {
    // SAFETY: the function these calls reached before (checked bytes).
    let original: extern "C" fn(u32, u32, u32) -> i32 =
        unsafe { std::mem::transmute(LUA_RAW_RUN_PROTECTED as usize) };
    let status = original(l, f, ud);
    if status != 0 {
        let _ = std::panic::catch_unwind(|| {
            let admitted = admit(mask::LUA_ERROR);
            let Some(body) = lua_error(l, status) else {
                return;
            };
            let message = match &body {
                EventBody::LuaError { message, .. } => message.clone(),
                _ => String::new(),
            };
            if admitted {
                record(body);
            }
            crate::debug::after_lua_error(&message);
        });
    }
    status
}

// --- Messages -----------------------------------------------------------------------

#[repr(C)]
struct Message {
    hash: u32,
    data: u32,
    sent: u8,
}

/// Per message hash: the window and how many of it were recorded in it.
static PER_HASH: Mutex<BTreeMap<u32, (u64, u32)>> = Mutex::new(BTreeMap::new());
/// Messages of one hash recorded per window of [`PER_HASH_WINDOW`] seconds:
/// the game sends about fifteen every frame (ticks, rendering) to hundreds
/// of listeners, and they would hide the rest.
const PER_HASH_RATE: u32 = 4;
const PER_HASH_WINDOW: u64 = 5;

/// Whether a message of `hash` may be recorded in this window.
fn admit_hash(hash: u32) -> bool {
    let s = second() / PER_HASH_WINDOW;
    let Ok(mut map) = PER_HASH.lock() else {
        return false;
    };
    if map.len() > 4096 {
        map.retain(|_, v| v.0 == s);
    }
    let e = map.entry(hash).or_insert((s, 0));
    if e.0 != s {
        *e = (s, 0);
    }
    e.1 += 1;
    e.1 <= PER_HASH_RATE
}

/// The innermost mark of a handler the bus is running on some thread: the
/// thunks of the three places that call a handler push
/// `{previous mark, listener}` under the return address of the game's call
/// and keep its address here while the handler runs. A message sent by that
/// handler — or by anything it calls — finds its listener through it.
///
/// One variable for every thread: the bus runs on the main thread, and a
/// mark is believed only if it lies in the stack of the thread that reads
/// it, above its current frame, and has one of the three return addresses
/// next to it.
static BUS_MARK: AtomicU32 = AtomicU32::new(0);

/// The listener whose handler is running on this thread and the hash of the
/// message it handles, from the innermost mark.
fn running_handler(m: &mut Mem) -> Option<(u32, u32)> {
    let mark = BUS_MARK.load(Ordering::Relaxed);
    if mark == 0 {
        return None;
    }
    let (base, limit): (u32, u32);
    // SAFETY: the stack bounds of this thread in its TEB.
    unsafe {
        std::arch::asm!(
            "mov {base}, fs:[4]",
            "mov {limit}, fs:[8]",
            base = out(reg) base,
            limit = out(reg) limit,
            options(nostack, readonly, preserves_flags)
        );
    }
    let here = std::ptr::from_ref(&mark) as usize as u32;
    if mark <= here || mark < limit || mark.checked_add(16)? > base {
        return None;
    }
    // SAFETY: four words of this thread's stack, above the current frame.
    let word = |i: u32| unsafe { std::ptr::read_volatile((mark + 4 * i) as usize as *const u32) };
    if !MESSAGE_CALL_RETURNS.contains(&word(2)) {
        return None;
    }
    let cause = m.u32(word(3)).unwrap_or(0);
    Some((word(1), cause))
}

/// Where a message comes from: the place that called the bus (`site`, 0 when
/// unknown), the handler that was running, the object its data points to.
fn message_from(m: &mut Mem, site: u32, data: u32, direct: bool) -> MessageFrom {
    let handler = running_handler(m);
    let object = |m: &mut Mem, at: u32| {
        crate::game::object_guid(m, at).filter(doctor_toolkit_live::is_object_guid)
    };
    let sender =
        handler.and_then(|(listener, _)| object(m, listener.wrapping_sub(OBJECT_LISTENER)));
    let data_object = (data >= 0x1_0000 && m.u32(data) == Some(MESSAGE_DATA_OBJECT_VTABLE))
        .then(|| m.u32(data + 4))
        .flatten()
        .filter(|&anchor| anchor >= 0x1_0000)
        .and_then(|anchor| object(m, anchor - MESSAGE_DATA_OBJECT_ANCHOR));
    MessageFrom {
        // Only code of the game: the module's own calls are not a place.
        site: if (0x0040_1000..0x00D5_4000).contains(&site) {
            site
        } else {
            0
        },
        cause: handler.map_or(0, |h| h.1),
        sender,
        data_object,
        direct,
    }
}

/// Records a message the bus is about to deliver (called by the thunk with
/// the bus's arguments and the stack as the bus got it: its return address
/// first).
extern "C" fn on_bus(message: *const Message, force: u32, frame: *const u32) {
    if message.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: the message the game passes to its bus.
        let (hash, data) = unsafe { ((*message).hash, (*message).data) };
        // SAFETY: the stack of the caller of the bus: the return address,
        // and — when that is the wrapper `0x0045C400(variable, data,
        // force)` — the wrapper's own return address past the two arguments
        // and its 0xC bytes of locals.
        let ret = unsafe {
            match *frame {
                MESSAGE_SEND_WRAPPER_RETURN => *frame.add(6),
                ret => ret,
            }
        };
        record_message(hash, data, force, ret.wrapping_sub(5));
        // Breakpoints and the free camera (stage 35), recorded or not: after
        // the record, so the journal shows the message a break stops at.
        crate::debug::before_message(hash, data, force & 0xFF != 0);
    });
}

/// Records a message the game delivers to one listener (called by the
/// thunks of `0x00408C31` and `0x00440819`). `frame` is the stack as
/// `0x00408BF0` left it at its call of the handler — the return address of
/// that function is 0x18 above — or 0 for the delayed queue, where the
/// place that sent the message is gone.
extern "C" fn on_direct(message: *const Message, listener: u32, frame: u32) {
    if message.is_null() || now_ms() > MESSAGES_UNTIL.load(Ordering::Relaxed) {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: the message the game is about to hand to the handler.
        let (hash, data) = unsafe { ((*message).hash, (*message).data) };
        if !admit_hash(hash) || !admit(mask::MESSAGE) {
            return;
        }
        let mut m = Mem::default();
        let mut site = 0;
        if frame != 0 {
            // The caller of 0x00408BF0, or of its two wrappers (their own
            // return address lies 0x1C further: three arguments, 0xC of
            // locals, and the call).
            let ret = m.u32(frame + 0x18).unwrap_or(0);
            let ret = if ret == 0x0040_8BAB || ret == 0x0040_8BDD {
                m.u32(frame + 0x34).unwrap_or(0)
            } else {
                ret
            };
            site = ret.wrapping_sub(5);
        }
        let from = message_from(&mut m, site, data, true);
        record(message_event(
            &mut m,
            hash,
            data,
            false,
            &[listener],
            MessageOrigin::Game,
            Some(from),
        ));
    });
}

/// Records a message of the bus while a client asks for them.
fn record_message(hash: u32, data: u32, force: u32, site: u32) {
    {
        if now_ms() > MESSAGES_UNTIL.load(Ordering::Relaxed) {
            return;
        }
        if !admit_hash(hash) {
            if let Some(r) = ring().as_mut() {
                r.lose(second(), mask::MESSAGE);
            }
            if let Ok(mut c) = COUNTS.try_lock() {
                c.get_or_insert_with(|| (Counts::default(), now_ms()))
                    .0
                    .add(hash);
            }
            return;
        }
        if !admit(mask::MESSAGE) {
            return;
        }
        let forced = force & 0xFF != 0;
        // SAFETY: the table and listeners the bus is about to walk itself.
        let listeners = unsafe { crate::game::bus_listeners(hash, !forced) };
        let mut m = Mem::default();
        let from = message_from(&mut m, site, data, false);
        record(message_event(
            &mut m,
            hash,
            data,
            forced,
            &listeners,
            if LINK_SENDING.load(Ordering::Relaxed) {
                MessageOrigin::Link
            } else {
                MessageOrigin::Game
            },
            Some(from),
        ));
    }
}

/// The event of a message delivered to `listeners`.
pub fn message_event(
    m: &mut Mem,
    hash: u32,
    data: u32,
    forced: bool,
    listeners: &[u32],
    origin: MessageOrigin,
    from: Option<MessageFrom>,
) -> EventBody {
    let mut receivers = Vec::new();
    for &listener in listeners {
        if receivers.len() >= MAX_RECEIVERS {
            break;
        }
        if let Some(g) = crate::game::object_guid(m, listener - OBJECT_LISTENER)
            && doctor_toolkit_live::is_object_guid(&g)
        {
            receivers.push(g);
        }
    }
    EventBody::Message {
        hash,
        data,
        forced,
        listeners: listeners.len() as u32,
        receivers,
        origin,
        from,
    }
}

/// Records a message the live link delivered itself (to one object).
pub fn record_link_message(m: &mut Mem, hash: u32, listeners: &[u32]) {
    if now_ms() <= MESSAGES_UNTIL.load(Ordering::Relaxed) && admit(mask::MESSAGE) {
        record(message_event(
            m,
            hash,
            0,
            false,
            listeners,
            MessageOrigin::Link,
            Some(MessageFrom {
                direct: true,
                ..MessageFrom::default()
            }),
        ));
    }
}

// The thunk the bus jumps to: saves every register, calls `on_bus(message,
// force)`, restores them, runs the five bytes the jump took
// (`push ecx; mov edx, [esp+8]`) and goes on at 0x00408A05.
global_asm!(
    ".globl _doctor_live_bus_thunk",
    "_doctor_live_bus_thunk:",
    "pushad",
    "pushfd",
    // Stack: flags, 8 registers (36 bytes), return address, message, force.
    "lea eax, [esp + 36]",
    "push eax",
    "push dword ptr [esp + 48]",
    "push dword ptr [esp + 48]",
    "call {on_bus}",
    "add esp, 12",
    "popfd",
    "popad",
    "push ecx",
    "mov edx, dword ptr [esp + 8]",
    "push {resume}",
    "ret",
    on_bus = sym on_bus,
    resume = const MESSAGE_SEND_RESUME,
);

// The thunks that stand for the game's three calls of a listener's handler
// (`thiscall handler(message)`, the handler removes its argument). On entry:
// ECX — the listener, the stack — the return address into the game and the
// message. Each leaves `{previous mark, listener}` on the stack under that
// return address, calls the handler with a copy of the argument, takes the
// mark off and returns past the argument, as the handler would have.
//
// `0x00408A8B` (`mov edx,[eax+4]; call edx`): EAX — the method table.
global_asm!(
    ".globl _doctor_live_call_broadcast_thunk",
    "_doctor_live_call_broadcast_thunk:",
    "push ecx",
    "push dword ptr [{mark}]",
    "mov dword ptr [{mark}], esp",
    "push dword ptr [esp + 12]",
    "call dword ptr [eax + 4]",
    "pop dword ptr [{mark}]",
    "add esp, 4",
    "ret 4",
    mark = sym BUS_MARK,
);

// `0x00408C31` (`and bl,1; call edx`): EAX — the method table. The delivery
// to one listener is recorded first: `on_direct(message, listener, frame)`.
global_asm!(
    ".globl _doctor_live_call_direct_thunk",
    "_doctor_live_call_direct_thunk:",
    "and bl, 1",
    "pushad",
    "pushfd",
    // Stack: flags, 8 registers (36 bytes), return address, message.
    "lea eax, [esp + 36]",
    "push eax",
    "push ecx",
    "push dword ptr [esp + 48]",
    "call {on_direct}",
    "add esp, 12",
    "popfd",
    "popad",
    "push ecx",
    "push dword ptr [{mark}]",
    "mov dword ptr [{mark}], esp",
    "push dword ptr [esp + 12]",
    "call dword ptr [eax + 4]",
    "pop dword ptr [{mark}]",
    "add esp, 4",
    "ret 4",
    on_direct = sym on_direct,
    mark = sym BUS_MARK,
);

// `0x00440819` (`and bl,1; call eax`): EAX — the handler.
global_asm!(
    ".globl _doctor_live_call_queue_thunk",
    "_doctor_live_call_queue_thunk:",
    "and bl, 1",
    "pushad",
    "pushfd",
    "push 0",
    "push ecx",
    "push dword ptr [esp + 48]",
    "call {on_direct}",
    "add esp, 12",
    "popfd",
    "popad",
    "push ecx",
    "push dword ptr [{mark}]",
    "mov dword ptr [{mark}], esp",
    "push dword ptr [esp + 12]",
    "call eax",
    "pop dword ptr [{mark}]",
    "add esp, 4",
    "ret 4",
    on_direct = sym on_direct,
    mark = sym BUS_MARK,
);

// The thunk at the start of `luaG_errormsg` (`L` in ESI): saves every
// register, calls `on_error(L)`, restores them, runs the five bytes the jump
// took (`mov eax, [esi+0x54]; test eax, eax` — the flags are what the next
// instruction tests) and goes on at 0x006338A5.
global_asm!(
    ".globl _doctor_live_error_thunk",
    "_doctor_live_error_thunk:",
    "pushad",
    "pushfd",
    "push esi",
    "call {on_error}",
    "add esp, 4",
    "popfd",
    "popad",
    "mov eax, dword ptr [esi + 0x54]",
    "test eax, eax",
    "push {resume}",
    "ret",
    on_error = sym on_error,
    resume = const LUA_ERRORMSG_RESUME,
);

unsafe extern "C" {
    fn doctor_live_bus_thunk();
    fn doctor_live_call_broadcast_thunk();
    fn doctor_live_call_direct_thunk();
    fn doctor_live_call_queue_thunk();
    fn doctor_live_error_thunk();
}

/// Writes a `jmp target` over five bytes of code at `at`.
///
/// # Safety
/// `at` must start five bytes of whole instructions without relative
/// operands, checked by `edition::check`, that the target runs instead.
unsafe fn jump(at: u32, target: u32) -> bool {
    let rel = target.wrapping_sub(at + 5);
    let mut code = [0xE9u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&rel.to_le_bytes());
    // SAFETY: see above.
    unsafe { win::patch(at, &code) }
}

/// Writes a `call target` over five bytes of code at `at`.
///
/// # Safety
/// As [`jump`]; the target must do what the five bytes did and return.
unsafe fn call(at: u32, target: u32) -> bool {
    let rel = target.wrapping_sub(at + 5);
    let mut code = [0xE8u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&rel.to_le_bytes());
    // SAFETY: see above.
    unsafe { win::patch(at, &code) }
}

/// Prints one line through the game's own import slot of
/// `OutputDebugStringA`: the journal must show it (a check of the hook).
pub fn self_test() {
    if RECORDING.load(Ordering::Relaxed) & mask::OUTPUT == 0 {
        return;
    }
    // SAFETY: the import slot, a function taking a C string.
    unsafe {
        let slot = std::ptr::read_volatile(IAT_OUTPUT_DEBUG_STRING as usize as *const u32);
        let f: extern "system" fn(*const u8) = std::mem::transmute(slot as usize);
        f(c"doctor_live: OutputDebugStringA goes to the journal"
            .as_ptr()
            .cast());
    }
}

/// What the Lua print callback held before the module set it.
pub fn print_callback_before() -> u32 {
    PRINT_ORIGINAL.load(Ordering::Relaxed)
}

// --- Installation -------------------------------------------------------------------

/// Installs the hooks of the journal (after the edition check passed).
/// Returns what it could not install.
pub fn install() -> Vec<&'static str> {
    let _ = STARTED.get_or_init(Instant::now);
    *ring() = Some(Ring::default());
    let mut failed = Vec::new();
    let mut recording = 0;
    // What the game prints.
    // SAFETY: the import slot (checked: its lookup names OutputDebugStringA)
    // is a 4-byte aligned pointer in the read-only data of the game.
    let slot = unsafe { std::ptr::read_volatile(IAT_OUTPUT_DEBUG_STRING as usize as *const u32) };
    OUTPUT_ORIGINAL.store(slot, Ordering::Relaxed);
    let hook = output_hook as extern "system" fn(*const u8) as usize as u32;
    // SAFETY: as above.
    if slot != 0 && unsafe { win::patch(IAT_OUTPUT_DEBUG_STRING, &hook.to_le_bytes()) } {
        recording |= mask::OUTPUT;
    } else {
        failed.push("OutputDebugStringA");
    }
    // Lua print: a data pointer the game reads before calling it.
    // SAFETY: a 4-byte aligned pointer in the data of the game, read by
    // the checked code of its print (0x0046BF56).
    let old = unsafe { std::ptr::read_volatile(LUA_PRINT_CALLBACK as usize as *const u32) };
    PRINT_ORIGINAL.store(old, Ordering::Relaxed);
    let print = print_hook as extern "C" fn(*const u8, i32, *const u8, *const u8) as usize as u32;
    // SAFETY: as above; `patch` makes the page writable for the store.
    if unsafe { win::patch(LUA_PRINT_CALLBACK, &print.to_le_bytes()) } {
        recording |= mask::OUTPUT;
    } else {
        failed.push("Lua print");
    }
    // Lua errors: three calls of luaD_rawrunprotected.
    let protected = protected_hook as extern "C" fn(u32, u32, u32) -> i32 as usize as u32;
    let mut lua_ok = true;
    for site in LUA_PROTECTED_CALLS {
        let rel = protected.wrapping_sub(site + 5);
        // SAFETY: call instructions checked by `edition::check`; only their
        // 4-byte targets change.
        lua_ok &= unsafe { win::patch(site + 1, &rel.to_le_bytes()) };
    }
    // The stack of run time errors, before Lua unwinds it.
    let error_thunk = doctor_live_error_thunk as unsafe extern "C" fn() as usize as u32;
    // SAFETY: the first instructions of luaG_errormsg, checked by
    // `edition::check`: `mov eax,[esi+0x54]; test eax,eax`.
    if !unsafe { jump(LUA_ERRORMSG, error_thunk) } {
        failed.push("Lua call stacks");
    }
    if lua_ok {
        recording |= mask::LUA_ERROR;
    } else {
        failed.push("Lua errors");
    }
    // The bus: a jump over its first five bytes.
    let thunk = doctor_live_bus_thunk as unsafe extern "C" fn() as usize as u32;
    // SAFETY: the first instructions of the bus, checked by
    // `edition::check` (`push ecx; mov edx,[esp+8]`, five bytes, no
    // relative operand); installed from DllMain, before the game's threads
    // run its code.
    if unsafe { jump(MESSAGE_SEND, thunk) } {
        recording |= mask::MESSAGE | mask::MESSAGE_COUNTS;
        // Who sends: the three calls of a listener's handler. All three or
        // none: a mark of one without the others would name a wrong sender.
        let thunks = [
            (
                MESSAGE_CALL_BROADCAST,
                doctor_live_call_broadcast_thunk as unsafe extern "C" fn() as usize as u32,
            ),
            (
                MESSAGE_CALL_DIRECT,
                doctor_live_call_direct_thunk as unsafe extern "C" fn() as usize as u32,
            ),
            (
                MESSAGE_CALL_QUEUE,
                doctor_live_call_queue_thunk as unsafe extern "C" fn() as usize as u32,
            ),
        ];
        let mut done = 0;
        for (at, thunk) in thunks {
            // SAFETY: five bytes of whole instructions without relative
            // operands at each place, checked by `edition::check`
            // (`mov edx,[eax+4]; call edx`, `and bl,1; call edx`,
            // `and bl,1; call eax`); installed from DllMain like the jump
            // above.
            if !unsafe { call(at, thunk) } {
                break;
            }
            done += 1;
        }
        if done == thunks.len() {
            recording |= mask::MESSAGE_SENDER;
        } else {
            // Put back what was replaced: the original instructions.
            let original: [&[u8]; 3] = [
                &[0x8B, 0x50, 0x04, 0xFF, 0xD2],
                &[0x80, 0xE3, 0x01, 0xFF, 0xD2],
                &[0x80, 0xE3, 0x01, 0xFF, 0xD0],
            ];
            for (i, (at, _)) in thunks.iter().enumerate().take(done) {
                // SAFETY: the bytes that were there.
                unsafe { win::patch(*at, original[i]) };
            }
            failed.push("message senders");
        }
    } else {
        failed.push("message bus");
    }
    RECORDING.store(recording, Ordering::Relaxed);
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_names_as_lua_shows_them() {
        assert_eq!(short_source("@scripts/mission.lua"), "scripts/mission.lua");
        assert_eq!(short_source("=doctor_live"), "doctor_live");
        assert_eq!(short_source("return 1"), "[string \"return 1\"]");
    }
}
