//! Draw calls of the frame (stage 101, `docs/analysis/renderer.md` §11).
//!
//! The game draws a frame with thousands of calls of `IDirect3DDevice9`
//! on its render thread. The module stands in the methods of the device's
//! vtable (the device is the game's, `*0x01205750`; the hooks go in at the
//! first present) and
//!
//! - counts what goes through ([`Counters`]): a line in the log every few
//!   seconds with the draws of a frame and the processor time of the
//!   render thread and of the whole process;
//! - on request writes every call of one whole frame with its arguments
//!   into `doctor_render_trace.bin` ([`trace`]): the request is a file
//!   `doctor_render_trace.go` next to the module (its text, if any, ends
//!   up in the name: `doctor_render_trace_<text>.bin`);
//!   `research/d3d9_frame_trace.py` reads the result;
//! - drops calls that set what is already set ([`filter`]);
//! - joins draws that follow each other with nothing changed between them
//!   into one draw ([`merge`]): the world is drawn as many short triangle
//!   strips of one vertex and index buffer under one material.
//!
//! Everything here runs on the thread that draws; a call from another
//! thread goes straight to the device.

use std::cell::UnsafeCell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use doctor_toolkit_live::render::{RenderConfig, addr, stitch_strip};

use crate::d3d::Object;
use crate::win;

/// Methods of `IDirect3DDevice9`.
pub const SLOTS: usize = 119;
/// Methods of other interfaces the module stands in (locks and queries).
const EXTRA: usize = 32;

/// State of the drawing thread; other threads never touch it.
struct Local<T>(UnsafeCell<T>);
// SAFETY: read and written on the drawing thread only (every hook checks
// the thread first); the tables of original methods are written before
// the hooks that read them go in.
unsafe impl<T> Sync for Local<T> {}

/// The device's own methods, by slot. The thunks jump through it.
static DOCTOR_RENDER_ORIGINAL: Local<[usize; SLOTS]> = Local(UnsafeCell::new([0; SLOTS]));
/// The own methods of the other interfaces, by the number of the thunk.
static DOCTOR_RENDER_EXTRA: Local<[usize; EXTRA]> = Local(UnsafeCell::new([0; EXTRA]));
static RENDER_THREAD: AtomicU32 = AtomicU32::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);
static STATE: Local<Option<State>> = Local(UnsafeCell::new(None));
/// Set while the module makes its own state block (the post-effects).
pub static OWN_STATE_BLOCK: AtomicBool = AtomicBool::new(false);
/// Set from any thread: the device's state changed behind the hooks.
static FORGET: AtomicBool = AtomicBool::new(false);

// The thunks: `call flush; jmp [original]`, 16 bytes each. Whatever the
// method and its arguments, the draws held back go to the device first.
core::arch::global_asm!(
    ".text",
    ".balign 16",
    ".globl _doctor_render_thunks",
    "_doctor_render_thunks:",
    ".set doctor_render_i, 0",
    ".rept 119",
    ".balign 16",
    "call {flush}",
    "jmp dword ptr [{original} + doctor_render_i]",
    ".set doctor_render_i, doctor_render_i + 4",
    ".endr",
    ".balign 16",
    ".globl _doctor_render_extra_thunks",
    "_doctor_render_extra_thunks:",
    ".set doctor_render_i, 0",
    ".rept 32",
    ".balign 16",
    "call {flush}",
    "jmp dword ptr [{extra} + doctor_render_i]",
    ".set doctor_render_i, doctor_render_i + 4",
    ".endr",
    flush = sym flush_any,
    original = sym DOCTOR_RENDER_ORIGINAL,
    extra = sym DOCTOR_RENDER_EXTRA,
);

unsafe extern "C" {
    fn doctor_render_thunks();
    fn doctor_render_extra_thunks();
}

#[inline(always)]
fn original(slot: usize) -> usize {
    // SAFETY: written once before any hook runs.
    unsafe { (*DOCTOR_RENDER_ORIGINAL.0.get())[slot] }
}

/// The state, on the render thread; `None` on any other thread — and a
/// call of another thread makes the filter forget what it knew. Draws
/// held back stay held back.
#[inline(always)]
fn state_raw() -> Option<&'static mut State> {
    // SAFETY: plain Win32 call; the state is the drawing thread's own.
    unsafe {
        if win::GetCurrentThreadId() != RENDER_THREAD.load(Ordering::Relaxed) {
            FORGET.store(true, Ordering::Relaxed);
            return None;
        }
        let state = (*STATE.0.get()).as_mut()?;
        if FORGET.load(Ordering::Relaxed) {
            FORGET.store(false, Ordering::Relaxed);
            state.flush();
            state.filter.forget();
        }
        Some(state)
    }
}

/// [`state_raw`], after the draws held back went to the device.
#[inline(always)]
fn state() -> Option<&'static mut State> {
    let state = state_raw()?;
    state.flush();
    Some(state)
}

/// Called by every thunk: the draws held back go to the device.
extern "C" fn flush_any() {
    let _ = state();
}

/// What went through since the last line of the log.
#[derive(Default, Clone, Copy)]
pub struct Counters {
    pub frames: u32,
    /// Draw calls the game made, and those that reached the device.
    pub draws: u64,
    pub draws_out: u64,
    pub primitives: u64,
    /// Every hooked call the game made, and those dropped by the filter.
    pub calls: u64,
    pub dropped: u64,
    /// Draws joined into one, and how many such joined draws.
    pub merged: u64,
    pub batches: u64,
}

struct State {
    counters: Counters,
    /// Counter, thread and process times at the last line.
    stamp: u64,
    thread_cpu: u64,
    process_cpu: u64,
    log_every: u64,
    frame: u32,
    trace: trace::Trace,
    filter: filter::Filter,
    merge: merge::Merge,
    /// What the configuration asks for; `draw_ab` turns both off and on.
    want_filter: bool,
    want_merge: bool,
    active: bool,
    /// `draw_ab`: seconds of each half, 0 — no alternation.
    alternate: u64,
    switched: u64,
}

impl State {
    /// The draws held back go to the device.
    #[inline(always)]
    fn flush(&mut self) {
        if self.merge.pending() {
            // SAFETY: on the drawing thread, with the device of the draws.
            unsafe { self.merge.flush(&mut self.counters, &mut self.trace) };
        }
    }

    /// A call that sets a state. `true`: it sets what is already set and
    /// is dropped. Otherwise the draws held back go first.
    #[inline(always)]
    fn setter(&mut self, same: bool) -> bool {
        self.counters.calls += 1;
        if same && self.filter.on {
            self.counters.dropped += 1;
            if self.trace.on {
                self.trace.record(trace::DROPPED, &[]);
            }
            return true;
        }
        self.flush();
        false
    }

    /// Whether the calls that set states are followed at all: with
    /// everything off (and no frame being written) they go straight on.
    #[inline(always)]
    fn busy(&self) -> bool {
        self.active || self.trace.on
    }

    fn apply(&mut self, active: bool) {
        self.flush();
        self.active = active;
        self.filter
            .set(active && (self.want_filter || self.want_merge));
        self.merge.on = active && self.want_merge && !self.merge.failed;
    }
}

/// 100 ns units of kernel and user time.
fn cpu(
    times: unsafe extern "system" fn(win::Handle, *mut u64, *mut u64, *mut u64, *mut u64) -> i32,
    handle: win::Handle,
) -> u64 {
    let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: fills the four FILETIMEs.
    unsafe { times(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    kernel + user
}

fn thread_cpu() -> u64 {
    // SAFETY: the pseudo-handle of this thread.
    cpu(win::GetThreadTimes, unsafe { win::GetCurrentThread() })
}

fn process_cpu() -> u64 {
    // SAFETY: the pseudo-handle of this process.
    cpu(win::GetProcessTimes, unsafe { win::GetCurrentProcess() })
}

/// Once a frame, on the thread that draws, before the copy of the frame
/// and `Present` (the module's hook of `0x0060D8FA`).
pub fn frame() {
    if !crate::config().draws() {
        return;
    }
    if !INSTALLED.load(Ordering::Relaxed) {
        // SAFETY: on the drawing thread, the device of the game.
        unsafe { install_hooks() };
        return;
    }
    // The state belongs to one thread for good: the game's render thread,
    // the first thread other than the main one (which loaded the module
    // and draws the loading screens itself) to present a frame. Handing
    // the state from thread to thread would race with a call still inside
    // a hook.
    // SAFETY: plain Win32 call.
    let thread = unsafe { win::GetCurrentThreadId() };
    if RENDER_THREAD.load(Ordering::SeqCst) == 0 {
        if thread == crate::MAIN_THREAD.load(Ordering::Relaxed) {
            return;
        }
        RENDER_THREAD.store(thread, Ordering::SeqCst);
        crate::log(&format!("draws: the render thread is {thread}"));
    }
    let Some(s) = state() else {
        return;
    };
    s.counters.frames += 1;
    s.frame += 1;
    let now = win::now();
    let freq = win::frequency();
    s.trace.frame_end(s.frame, &s.counters);
    if s.frame % 2 == 0 && crate::config().draw_trace {
        s.trace.poll();
        // Test aid: a file next to the module turns merging and the filter
        // off for as long as it is there.
        if s.alternate == 0 {
            let off = crate::module_dir().join("doctor_render_draws.off").exists();
            if off == s.active {
                s.apply(!off);
            }
        }
    }
    s.merge.frame(s.frame);
    let window = now.saturating_sub(s.stamp);
    let switch = s.alternate != 0 && now.saturating_sub(s.switched) >= s.alternate * freq;
    if (s.log_every != 0 && window >= s.log_every * freq) || switch {
        let (thread, process) = (thread_cpu(), process_cpu());
        let c = s.counters;
        if c.frames > 0 && window > 0 {
            let n = c.frames as f64;
            crate::log(&format!(
                "draws: {}; {} frames in {:.2} s ({:.3} ms a frame); a frame: {:.0} draws ({:.0} to the device, {:.0} joined into {:.0}), {:.0} primitives, {:.0} calls ({:.0} dropped); render thread {:.3} ms, process {:.3} ms of processor",
                match (s.merge.on, s.filter.on) {
                    (true, _) => "merge on",
                    (false, true) => "filter on",
                    (false, false) => "off",
                },
                c.frames,
                window as f64 / freq as f64,
                window as f64 * 1000.0 / freq as f64 / n,
                c.draws as f64 / n,
                c.draws_out as f64 / n,
                c.merged as f64 / n,
                c.batches as f64 / n,
                c.primitives as f64 / n,
                c.calls as f64 / n,
                c.dropped as f64 / n,
                thread.saturating_sub(s.thread_cpu) as f64 / 10_000.0 / n,
                process.saturating_sub(s.process_cpu) as f64 / 10_000.0 / n,
            ));
        }
        s.counters = Counters::default();
        s.stamp = now;
        s.thread_cpu = thread;
        s.process_cpu = process;
        if switch {
            let active = !s.active;
            s.apply(active);
            s.switched = now;
        }
    }
    s.trace.frame_start(s.frame);
}

/// The state of the device changed behind the hooks — `Reset`, a state
/// block applied by the post-effects: the filter forgets what it knew.
/// From any thread.
pub fn forget() {
    FORGET.store(true, Ordering::Relaxed);
}

/// Makes `count` words at `at` writable, writes with `write`, and puts the
/// protection back.
///
/// # Safety
/// `at` must be a vtable of at least `count` methods.
unsafe fn patch_table(at: *mut usize, count: usize, write: impl FnOnce(*mut usize)) -> bool {
    let mut old = 0u32;
    let bytes = count * std::mem::size_of::<usize>();
    // SAFETY: the caller's promise; the protection is restored.
    unsafe {
        if win::VirtualProtect(at as *mut c_void, bytes, win::PAGE_READWRITE, &mut old) == 0 {
            return false;
        }
        write(at);
        let mut ignored = 0u32;
        win::VirtualProtect(at as *mut c_void, bytes, old, &mut ignored);
    }
    true
}

/// Puts the hooks into the vtable of the game's device.
unsafe fn install_hooks() {
    // SAFETY: data of the game proved by the checked places; the vtable of
    // a live device has `SLOTS` methods.
    unsafe {
        let device = win::read::<u32>(addr::DEVICE) as usize as Object;
        if device.is_null() {
            return;
        }
        let vtable = *(device as *const *mut usize);
        let table = &mut *DOCTOR_RENDER_ORIGINAL.0.get();
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = *vtable.add(i);
        }
        let config = crate::config();
        let now = win::now();
        let mut state = State {
            counters: Counters::default(),
            stamp: now,
            thread_cpu: thread_cpu(),
            process_cpu: process_cpu(),
            log_every: u64::from(config.draw_log),
            frame: 0,
            trace: trace::Trace::new(),
            filter: filter::Filter::new(false),
            merge: merge::Merge::new(config.draw_merge),
            want_filter: config.draw_filter,
            want_merge: config.draw_merge > 0,
            active: true,
            alternate: u64::from(config.draw_ab),
            switched: now,
        };
        state.apply(true);
        if config.draw_merge > 0 {
            // Queries made before the hooks went in bracket draws too
            // (the game asks whether a zone is seen): one query of each
            // kind gives their vtables.
            let create: extern "system" fn(Object, u32, *mut Object) -> i32 =
                std::mem::transmute(*vtable.add(118));
            for kind in [9u32, 8] {
                let mut query: Object = std::ptr::null_mut();
                if create(device, kind, &mut query) >= 0 && !query.is_null() {
                    state.merge.watch(query, 6);
                    crate::d3d::release(query);
                }
            }
        }
        *STATE.0.get() = Some(state);
        let hooks = hooks::table();
        // With merging every other method of the device lets the draws
        // held back go first.
        let all = config.draw_merge > 0;
        let patched = patch_table(vtable, SLOTS, |vtable| {
            if all {
                for slot in 0..SLOTS {
                    *vtable.add(slot) = doctor_render_thunks as *const () as usize + 16 * slot;
                }
            }
            for &(slot, hook) in &hooks {
                *vtable.add(slot) = hook;
            }
        });
        INSTALLED.store(true, Ordering::Relaxed);
        if all
            && !merge::prepare(device)
            && let Some(state) = (*STATE.0.get()).as_mut()
        {
            state.merge.failed = true;
            state.merge.on = false;
        }
        if !patched {
            crate::log("draws: the vtable of the device cannot be written; off");
            *STATE.0.get() = None;
            return;
        }
        crate::log(&format!(
            "draws: {} methods of the device {device:p} hooked on thread {}; filter {}, merge {}",
            if all { SLOTS } else { hooks.len() },
            win::GetCurrentThreadId(),
            if config.draw_filter || all {
                "on"
            } else {
                "off"
            },
            if all {
                format!(
                    "on (runs of {} draws and more)",
                    merge::least(config.draw_merge)
                )
            } else {
                "off".to_owned()
            },
        ));
    }
}

pub fn install(config: &RenderConfig, done: &mut Vec<String>) {
    if !config.draws() {
        return;
    }
    let mut what = Vec::new();
    if config.draw_merge > 0 {
        what.push(format!(
            "draws with nothing changed between them are joined (runs of {} and more)",
            merge::least(config.draw_merge)
        ));
    }
    if config.draw_filter || config.draw_merge > 0 {
        what.push("calls that change nothing are dropped".to_owned());
    }
    if config.draw_trace {
        what.push("test: a frame is written on request".to_owned());
    }
    if config.draw_ab > 0 {
        what.push(format!("test: on and off every {} s", config.draw_ab));
    }
    done.push(format!(
        "draw calls: hooks on the device at the first present; {}",
        what.join("; ")
    ));
}

/// Addresses in the game's code among the words of the stack above
/// `from`: the callers of a method of the device, nearest first.
///
/// # Safety
/// `from` must point into the stack of the current thread.
unsafe fn callers(from: *const u32, out: &mut [u32; 8]) -> usize {
    let mut n = 0;
    // SAFETY: the stack between `from` and its base (TEB `+4`) is
    // readable; a candidate lies in the game's code, whose bytes before it
    // are readable too.
    unsafe {
        let base: u32;
        core::arch::asm!("mov {}, fs:[4]", out(reg) base, options(nostack, readonly));
        let base = base as usize as *const u32;
        let mut p = from;
        let end = from.add(160).min(base);
        while p < end && n < out.len() {
            let v = *p;
            if (0x0040_1010..0x00D5_4000).contains(&v) {
                let b = |back: u32| *((v - back) as usize as *const u8);
                let call = b(5) == 0xE8
                    || (b(2) == 0xFF && (b(1) & 0x38) == 0x10)
                    || (b(3) == 0xFF && (b(2) & 0x38) == 0x10)
                    || (b(6) == 0xFF && (b(5) & 0x38) == 0x10)
                    || (b(7) == 0xFF && (b(6) & 0x38) == 0x10);
                if call {
                    out[n] = v;
                    n += 1;
                }
            }
            p = p.add(1);
        }
    }
    n
}

/// The method at `slot` of the vtable of `object`.
///
/// # Safety
/// `object` must be a live COM object with at least `slot + 1` methods.
#[inline(always)]
unsafe fn method(object: Object, slot: usize) -> usize {
    // SAFETY: the caller's promise.
    unsafe { *(*(object as *const *const usize)).add(slot) }
}

/// Recording of one whole frame.
pub mod trace {
    use super::*;

    /// Records beside the calls (a call's record is its slot of the
    /// vtable).
    pub const FRAME: u32 = 0x100;
    pub const SHADER: u32 = 0x101;
    pub const DECLARATION: u32 = 0x102;
    pub const VERTEX_BUFFER: u32 = 0x103;
    pub const INDEX_BUFFER: u32 = 0x104;
    pub const SURFACE: u32 = 0x105;
    pub const TEXTURE: u32 = 0x106;
    pub const TEXTURE_TABLE: u32 = 0x107;
    pub const CALLERS: u32 = 0x108;
    pub const FRAME_END: u32 = 0x109;
    /// A call the filter dropped: the record that follows did not reach
    /// the device.
    pub const DROPPED: u32 = 0x10A;
    /// The draws recorded before went to the device as one: their number,
    /// the primitive type, the indices and the primitives of the one draw.
    pub const MERGED: u32 = 0x10B;

    const MAGIC: [u32; 2] = [0x4432_4647, 0x3157_4152]; // "GF2DRAW1"

    pub struct Trace {
        pub on: bool,
        armed: bool,
        name: String,
        buf: Vec<u32>,
        seen: HashSet<(u32, usize)>,
    }

    impl Trace {
        pub fn new() -> Self {
            Self {
                on: false,
                armed: false,
                name: String::new(),
                buf: Vec::new(),
                seen: HashSet::new(),
            }
        }

        /// Looks for the request file.
        pub fn poll(&mut self) {
            if self.on || self.armed || !crate::config().draw_trace {
                return;
            }
            let go = crate::module_dir().join("doctor_render_trace.go");
            let Ok(text) = std::fs::read_to_string(&go) else {
                return;
            };
            self.name = text
                .trim()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(40)
                .collect();
            self.armed = true;
        }

        pub fn frame_start(&mut self, frame: u32) {
            if !self.armed {
                return;
            }
            self.armed = false;
            self.on = true;
            self.buf.clear();
            self.buf.reserve(8 << 20);
            self.seen.clear();
            self.buf.extend_from_slice(&MAGIC);
            self.record(FRAME, &[frame]);
            // The texture table: which entry holds which texture.
            // SAFETY: the table of the game (checked address), 4096
            // entries of 0x38 bytes.
            let table = unsafe {
                std::slice::from_raw_parts(
                    addr::TEXTURES as usize as *const u32,
                    (addr::TEXTURE_COUNT * addr::TEXTURE_ENTRY / 4) as usize,
                )
            };
            self.record(TEXTURE_TABLE, table);
        }

        pub fn frame_end(&mut self, frame: u32, counters: &Counters) {
            if !self.on {
                return;
            }
            self.on = false;
            self.record(FRAME_END, &[frame, counters.frames]);
            let file = if self.name.is_empty() {
                "doctor_render_trace.bin".to_owned()
            } else {
                format!("doctor_render_trace_{}.bin", self.name)
            };
            let dir = crate::module_dir();
            let bytes: Vec<u8> = self.buf.iter().flat_map(|w| w.to_le_bytes()).collect();
            let ok = std::fs::write(dir.join(&file), &bytes).is_ok();
            let _ = std::fs::remove_file(dir.join("doctor_render_trace.go"));
            crate::log(&format!(
                "draws: frame {frame} written to {file}: {} bytes{}",
                bytes.len(),
                if ok { "" } else { " (failed)" }
            ));
            self.buf = Vec::new();
            self.seen = HashSet::new();
        }

        #[inline]
        pub fn record(&mut self, op: u32, words: &[u32]) {
            self.buf.push(op);
            self.buf.push(words.len() as u32);
            self.buf.extend_from_slice(words);
        }

        /// A call with the words `head` and then `count` words at `data`.
        ///
        /// # Safety
        /// `data` must be readable for `count` words (or `count` zero).
        pub unsafe fn record_data(
            &mut self,
            op: u32,
            head: &[u32],
            data: *const u32,
            count: usize,
        ) {
            self.buf.push(op);
            self.buf.push((head.len() + count) as u32);
            self.buf.extend_from_slice(head);
            if count > 0 && !data.is_null() {
                // SAFETY: the caller's promise.
                self.buf
                    .extend_from_slice(unsafe { std::slice::from_raw_parts(data, count) });
            } else {
                self.buf.extend(std::iter::repeat_n(0, count));
            }
        }

        /// What an object is, the first time the frame uses it.
        ///
        /// # Safety
        /// `object` must be null or a live object of the kind `kind`.
        pub unsafe fn describe(&mut self, kind: u32, object: Object) {
            if object.is_null() || !self.seen.insert((kind, object as usize)) {
                return;
            }
            let ptr = object as usize as u32;
            // SAFETY: methods of the interfaces by their slots; every
            // output is a local buffer of the documented size.
            unsafe {
                match kind {
                    SHADER => {
                        // IDirect3D{Vertex,Pixel}Shader9::GetFunction.
                        let f: extern "system" fn(Object, *mut c_void, *mut u32) -> i32 =
                            std::mem::transmute(method(object, 4));
                        let mut size = 0u32;
                        if f(object, std::ptr::null_mut(), &mut size) >= 0 && size < (1 << 20) {
                            let mut code = vec![0u32; size.div_ceil(4) as usize];
                            if f(object, code.as_mut_ptr() as *mut c_void, &mut size) >= 0 {
                                self.record_data(SHADER, &[ptr], code.as_ptr(), code.len());
                            }
                        }
                    }
                    DECLARATION => {
                        let f: extern "system" fn(Object, *mut u32, *mut u32) -> i32 =
                            std::mem::transmute(method(object, 4));
                        let mut elements = [0u32; 2 * 66];
                        let mut count = 0u32;
                        if f(object, elements.as_mut_ptr(), &mut count) >= 0 {
                            let n = (count as usize).min(65) * 2;
                            self.record_data(DECLARATION, &[ptr], elements.as_ptr(), n);
                        }
                    }
                    VERTEX_BUFFER | INDEX_BUFFER => {
                        let f: extern "system" fn(Object, *mut u32) -> i32 =
                            std::mem::transmute(method(object, 13));
                        let mut desc = [0u32; 6];
                        if f(object, desc.as_mut_ptr()) >= 0 {
                            self.record_data(kind, &[ptr], desc.as_ptr(), 6);
                        }
                    }
                    SURFACE => {
                        let f: extern "system" fn(Object, *mut u32) -> i32 =
                            std::mem::transmute(method(object, 12));
                        let mut desc = [0u32; 8];
                        if f(object, desc.as_mut_ptr()) >= 0 {
                            self.record_data(SURFACE, &[ptr], desc.as_ptr(), 8);
                        }
                    }
                    TEXTURE => {
                        let kind_of: extern "system" fn(Object) -> u32 =
                            std::mem::transmute(method(object, 10));
                        let levels: extern "system" fn(Object) -> u32 =
                            std::mem::transmute(method(object, 13));
                        let t = kind_of(object);
                        let mut desc = [0u32; 8];
                        if t == 3 || t == 5 {
                            let f: extern "system" fn(Object, u32, *mut u32) -> i32 =
                                std::mem::transmute(method(object, 17));
                            f(object, 0, desc.as_mut_ptr());
                        }
                        self.record_data(TEXTURE, &[ptr, t, levels(object)], desc.as_ptr(), 8);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Dropping calls that set what is already set.
pub mod filter {
    const RENDER_STATES: usize = 256;
    const SAMPLERS: usize = 20;
    const SAMPLER_STATES: usize = 14;
    const CONSTANTS: usize = 256;
    const STREAMS: usize = 16;

    /// What the device holds, as far as the calls seen tell.
    pub struct Filter {
        pub on: bool,
        /// Off for good: something sets the state behind the hooks.
        poisoned: bool,
        render: [Option<u32>; RENDER_STATES],
        sampler: [[Option<u32>; SAMPLER_STATES]; SAMPLERS],
        texture: [Option<usize>; SAMPLERS],
        vs: Option<usize>,
        ps: Option<usize>,
        declaration: Option<usize>,
        indices: Option<usize>,
        stream: [Option<(usize, u32, u32)>; STREAMS],
        vs_constants: Box<[[u32; 4]; CONSTANTS]>,
        vs_known: [bool; CONSTANTS],
        ps_constants: Box<[[u32; 4]; CONSTANTS]>,
        ps_known: [bool; CONSTANTS],
    }

    impl Filter {
        pub fn new(on: bool) -> Self {
            Self {
                on,
                poisoned: false,
                render: [None; RENDER_STATES],
                sampler: [[None; SAMPLER_STATES]; SAMPLERS],
                texture: [None; SAMPLERS],
                vs: None,
                ps: None,
                declaration: None,
                indices: None,
                stream: [None; STREAMS],
                vs_constants: Box::new([[0; 4]; CONSTANTS]),
                vs_known: [false; CONSTANTS],
                ps_constants: Box::new([[0; 4]; CONSTANTS]),
                ps_known: [false; CONSTANTS],
            }
        }

        pub fn set(&mut self, on: bool) {
            self.forget();
            self.on = on && !self.poisoned;
        }

        /// Nothing is known any more.
        pub fn forget(&mut self) {
            let (on, poisoned) = (self.on, self.poisoned);
            *self = Self::new(on);
            self.poisoned = poisoned;
        }

        pub fn forget_declaration(&mut self) {
            self.declaration = None;
        }

        /// The index buffer the game set last, if a call told.
        pub fn current_indices(&self) -> Option<usize> {
            self.indices
        }

        /// Turns the filter off for good.
        pub fn poison(&mut self, why: &str) {
            if !self.poisoned {
                self.poisoned = true;
                if self.on {
                    crate::log(&format!("draws: {why}; filter and merge are off"));
                }
                self.on = false;
            }
        }

        #[inline]
        fn same<T: PartialEq + Copy>(slot: &mut Option<T>, value: T) -> bool {
            if *slot == Some(value) {
                return true;
            }
            *slot = Some(value);
            false
        }

        /// `true`: the render state already has this value.
        #[inline]
        pub fn render_state(&mut self, state: u32, value: u32) -> bool {
            // Point size and adaptive tessellation carry driver commands
            // (depth resolve, alpha to coverage) that act on every call.
            if matches!(state, 154 | 180 | 181) {
                return false;
            }
            match self.render.get_mut(state as usize) {
                Some(slot) => Self::same(slot, value),
                None => false,
            }
        }

        /// Sampler index of the device: 0–15, and 257–260 of the vertex
        /// shader as 16–19.
        #[inline]
        fn sampler_index(sampler: u32) -> Option<usize> {
            match sampler {
                0..=15 => Some(sampler as usize),
                257..=260 => Some(sampler as usize - 257 + 16),
                _ => None,
            }
        }

        #[inline]
        pub fn sampler_state(&mut self, sampler: u32, state: u32, value: u32) -> bool {
            let Some(i) = Self::sampler_index(sampler) else {
                return false;
            };
            match self.sampler[i].get_mut(state as usize) {
                Some(slot) => Self::same(slot, value),
                None => false,
            }
        }

        #[inline]
        pub fn texture(&mut self, sampler: u32, texture: usize) -> bool {
            match Self::sampler_index(sampler) {
                Some(i) => Self::same(&mut self.texture[i], texture),
                None => false,
            }
        }

        #[inline]
        pub fn vertex_shader(&mut self, shader: usize) -> bool {
            Self::same(&mut self.vs, shader)
        }

        #[inline]
        pub fn pixel_shader(&mut self, shader: usize) -> bool {
            Self::same(&mut self.ps, shader)
        }

        #[inline]
        pub fn declaration(&mut self, declaration: usize) -> bool {
            Self::same(&mut self.declaration, declaration)
        }

        #[inline]
        pub fn indices(&mut self, buffer: usize) -> bool {
            Self::same(&mut self.indices, buffer)
        }

        #[inline]
        pub fn stream(&mut self, index: u32, buffer: usize, offset: u32, stride: u32) -> bool {
            match self.stream.get_mut(index as usize) {
                Some(slot) => Self::same(slot, (buffer, offset, stride)),
                None => false,
            }
        }

        /// `true`: every register of the range already holds these bits.
        ///
        /// # Safety
        /// `data` must be readable for `count` vectors of four floats.
        #[inline]
        unsafe fn constants(
            held: &mut [[u32; 4]; CONSTANTS],
            known: &mut [bool; CONSTANTS],
            start: u32,
            data: *const [u32; 4],
            count: u32,
        ) -> bool {
            let (start, count) = (start as usize, count as usize);
            if data.is_null() || count == 0 || start + count > CONSTANTS {
                return false;
            }
            // SAFETY: the caller's promise.
            let new = unsafe { std::slice::from_raw_parts(data, count) };
            let same = known[start..start + count].iter().all(|k| *k)
                && held[start..start + count] == *new;
            if !same {
                held[start..start + count].copy_from_slice(new);
                known[start..start + count].fill(true);
            }
            same
        }

        /// # Safety
        /// `data` must be readable for `count` vectors of four floats.
        #[inline]
        pub unsafe fn vs_constants(
            &mut self,
            start: u32,
            data: *const [u32; 4],
            count: u32,
        ) -> bool {
            // SAFETY: the caller's promise.
            unsafe {
                Self::constants(
                    &mut self.vs_constants,
                    &mut self.vs_known,
                    start,
                    data,
                    count,
                )
            }
        }

        /// # Safety
        /// `data` must be readable for `count` vectors of four floats.
        #[inline]
        pub unsafe fn ps_constants(
            &mut self,
            start: u32,
            data: *const [u32; 4],
            count: u32,
        ) -> bool {
            // SAFETY: the caller's promise.
            unsafe {
                Self::constants(
                    &mut self.ps_constants,
                    &mut self.ps_known,
                    start,
                    data,
                    count,
                )
            }
        }
    }
}

/// Joining draws that follow each other with nothing changed between
/// them.
///
/// An indexed draw of a triangle strip or list is held back. The next one
/// joins it when no other call came between them — any call that reaches
/// the device, a lock of a buffer, texture or surface, or a query lets
/// the draws held back go first. A run of enough draws goes to the device
/// as one draw: its indices are copied from a copy of the game's index
/// buffer into an index buffer of the module (once: the next frame finds
/// the run there) — strips stitched with
/// repeated indices (triangles of no area, which draw nothing), with one
/// more repeat where the next strip would start on an odd index and turn
/// inside out. The triangles, their order and their vertices are the
/// same, so the picture is.
pub mod merge {
    use super::*;

    /// Indices of the module's index buffer (16 bits each): two frames'
    /// worth of joined runs of the densest scenes.
    const ARENA: u32 = 1 << 21;
    /// Draws of one run at most.
    const MOST: usize = 1024;
    const D3DLOCK_READONLY: u32 = 0x10;
    const D3DUSAGE_WRITEONLY: u32 = 0x8;
    const D3DUSAGE_DYNAMIC: u32 = 0x200;
    const D3DFMT_INDEX16: u32 = 101;
    const TRIANGLE_LIST: u32 = 4;
    const TRIANGLE_STRIP: u32 = 5;

    /// The shortest run joined for `draw_merge=N`: 1 means the default.
    pub fn least(setting: u32) -> usize {
        match setting {
            0 | 1 => 3,
            n => n.min(64) as usize,
        }
    }

    const D3DPOOL_MANAGED: u32 = 1;

    /// The module's index buffer, of the `MANAGED` pool: made once when the
    /// hooks go in, it lives through resets of the device.
    static ARENA_BUFFER: AtomicUsize = AtomicUsize::new(0);
    /// Index buffers locked for writing since their copy was taken.
    static WRITTEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static WRITTEN_ANY: AtomicBool = AtomicBool::new(false);
    /// The vtable of index buffers and its own `Lock`.
    static INDEX_VTABLE: AtomicUsize = AtomicUsize::new(0);
    static INDEX_LOCK: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy)]
    struct Draw {
        min: u32,
        vertices: u32,
        start: u32,
        count: u32,
    }

    /// A copy of an index buffer of the game (16-bit indices), or `None`
    /// for one that cannot be read.
    struct Copy_ {
        indices: Option<Vec<u16>>,
        used: u32,
    }

    /// A run in the module's buffer: what it was made of (the game's index
    /// buffer, the primitive type, then start and count of every draw),
    /// where it lies and the vertices it spans.
    struct Run {
        key: Box<[u32]>,
        at: u32,
        total: u32,
        low: u32,
        span: u32,
    }

    fn hash(words: &[u32]) -> u64 {
        // FNV-1a over the words.
        words.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, w| {
            (h ^ u64::from(*w)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    }

    pub struct Merge {
        pub on: bool,
        /// Off for good after failures.
        pub failed: bool,
        least: usize,
        device: Object,
        kind: u32,
        base: i32,
        indices: usize,
        queue: Vec<Draw>,
        copies: HashMap<usize, Copy_>,
        scratch: Vec<u16>,
        /// Runs already in the module's buffer, by the hash of their key.
        runs: HashMap<u64, Run>,
        key: Vec<u32>,
        arena_at: u32,
        frame: u32,
        failures: u32,
        /// Vtables whose locks already let the draws held back go first.
        watched: Vec<usize>,
    }

    type LockBuffer = extern "system" fn(Object, u32, u32, *mut *mut c_void, u32) -> i32;

    impl Merge {
        pub fn new(setting: u32) -> Self {
            Self {
                on: false,
                failed: false,
                least: least(setting),
                device: std::ptr::null_mut(),
                kind: 0,
                base: 0,
                indices: 0,
                queue: Vec::with_capacity(MOST),
                copies: HashMap::new(),
                scratch: Vec::new(),
                runs: HashMap::new(),
                key: Vec::new(),
                arena_at: 0,
                frame: 0,
                failures: 0,
                watched: Vec::new(),
            }
        }

        #[inline(always)]
        pub fn pending(&self) -> bool {
            !self.queue.is_empty()
        }

        /// Once a frame: copies of index buffers not used for a long time
        /// go away.
        pub fn frame(&mut self, frame: u32) {
            self.frame = frame;
            self.written();
            if frame.is_multiple_of(1024) {
                self.copies.retain(|_, c| frame.wrapping_sub(c.used) < 2048);
            }
        }

        /// Holds an indexed draw back. `false`: it is not one the module
        /// joins, and the caller sends it to the device.
        ///
        /// # Safety
        /// On the drawing thread, `device` the game's device.
        #[inline]
        pub unsafe fn hold(
            &mut self,
            counters: &mut Counters,
            trace: &mut trace::Trace,
            device: Object,
            indices: Option<usize>,
            kind: u32,
            base: i32,
            draw: [u32; 4],
        ) -> bool {
            let [min, vertices, start, count] = draw;
            let Some(indices) = indices.filter(|i| *i != 0) else {
                return false;
            };
            if !self.on || !matches!(kind, TRIANGLE_LIST | TRIANGLE_STRIP) || count == 0 {
                return false;
            }
            if self.pending()
                && (self.kind != kind
                    || self.base != base
                    || self.indices != indices
                    || self.device != device
                    || self.queue.len() >= MOST)
            {
                // SAFETY: the caller's promise.
                unsafe { self.flush(counters, trace) };
            }
            self.device = device;
            self.kind = kind;
            self.base = base;
            self.indices = indices;
            self.queue.push(Draw {
                min,
                vertices,
                start,
                count,
            });
            true
        }

        /// Sends the draws held back to the device: as one draw when they
        /// are a run long enough, one by one otherwise.
        ///
        /// # Safety
        /// On the drawing thread; the device of the draws is live.
        pub unsafe fn flush(&mut self, counters: &mut Counters, trace: &mut trace::Trace) {
            let n = self.queue.len();
            if n == 0 {
                return;
            }
            // SAFETY: the caller's promise.
            unsafe {
                let joined = if n >= self.least { self.joined() } else { None };
                match joined {
                    Some((indices, primitives)) => {
                        counters.draws_out += 1;
                        counters.merged += n as u64;
                        counters.batches += 1;
                        if trace.on {
                            trace
                                .record(trace::MERGED, &[n as u32, self.kind, indices, primitives]);
                        }
                    }
                    None => {
                        let draw: extern "system" fn(Object, u32, i32, u32, u32, u32, u32) -> i32 =
                            std::mem::transmute(original(82));
                        for d in &self.queue {
                            draw(
                                self.device,
                                self.kind,
                                self.base,
                                d.min,
                                d.vertices,
                                d.start,
                                d.count,
                            );
                        }
                        counters.draws_out += n as u64;
                    }
                }
            }
            self.queue.clear();
        }

        fn fail(&mut self, why: &str) -> Option<(u32, u32)> {
            self.failures += 1;
            if self.failures <= 3 {
                crate::log(&format!("draws: {why}; the draws go one by one"));
            }
            if self.failures >= 16 && !self.failed {
                self.failed = true;
                self.on = false;
                crate::log("draws: merge is off after 16 failures");
            }
            None
        }

        /// Index buffers of the game written since the last look: their
        /// copies and the runs made of them go.
        fn written(&mut self) {
            if WRITTEN_ANY.swap(false, Ordering::Relaxed)
                && let Ok(mut written) = WRITTEN.lock()
            {
                for b in written.drain(..) {
                    self.copies.remove(&b);
                    self.runs.retain(|_, r| r.key[0] != b as u32);
                }
            }
        }

        /// The copy of the game's index buffer `buffer`.
        ///
        /// # Safety
        /// `buffer` must be a live index buffer.
        unsafe fn copy(&mut self, buffer: usize) -> Option<&[u16]> {
            let frame = self.frame;
            let lock = INDEX_LOCK.load(Ordering::Relaxed);
            let entry = self.copies.entry(buffer).or_insert_with(|| {
                let object = buffer as Object;
                let mut indices = None;
                // SAFETY: a live index buffer: GetDesc, Lock for reading
                // (through its own Lock, not the module's hook), Unlock.
                unsafe {
                    let describe: extern "system" fn(Object, *mut u32) -> i32 =
                        std::mem::transmute(method(object, 13));
                    let mut desc = [0u32; 5];
                    // Format, type, usage, pool, size. A buffer of the
                    // DEFAULT pool made write-only cannot be read back.
                    let readable = describe(object, desc.as_mut_ptr()) >= 0
                        && desc[0] == D3DFMT_INDEX16
                        && desc[2] & D3DUSAGE_DYNAMIC == 0
                        && (desc[3] != 0 || desc[2] & D3DUSAGE_WRITEONLY == 0)
                        && *(object as *const usize) == INDEX_VTABLE.load(Ordering::Relaxed)
                        && lock != 0;
                    if readable {
                        let lock: LockBuffer = std::mem::transmute(lock);
                        let unlock: extern "system" fn(Object) -> i32 =
                            std::mem::transmute(method(object, 12));
                        let mut data: *mut c_void = std::ptr::null_mut();
                        if lock(object, 0, 0, &mut data, D3DLOCK_READONLY) >= 0 {
                            if !data.is_null() {
                                let words = desc[4] as usize / 2;
                                indices = Some(
                                    std::slice::from_raw_parts(data as *const u16, words).to_vec(),
                                );
                            }
                            unlock(object);
                        }
                    }
                }
                Copy_ {
                    indices,
                    used: frame,
                }
            });
            entry.used = frame;
            if crate::config().draw_debug & 2 != 0
                && let Some(held) = entry.indices.as_deref()
            {
                // SAFETY: as when the copy was taken.
                unsafe {
                    let object = buffer as Object;
                    let lock: LockBuffer = std::mem::transmute(lock);
                    let unlock: extern "system" fn(Object) -> i32 =
                        std::mem::transmute(method(object, 12));
                    let mut data: *mut c_void = std::ptr::null_mut();
                    if lock(object, 0, 0, &mut data, D3DLOCK_READONLY) >= 0 && !data.is_null() {
                        let now = std::slice::from_raw_parts(data as *const u16, held.len());
                        let differ = now.iter().zip(held).filter(|(a, b)| a != b).count();
                        unlock(object);
                        if differ > 0 {
                            crate::log(&format!(
                                "draws: debug: the copy of index buffer {buffer:#x} differs in {differ} of {} indices",
                                held.len()
                            ));
                        }
                    }
                }
            }
            entry.indices.as_deref()
        }

        /// Draws the run as one draw: the first index in the module's
        /// buffer and the primitives; `None` when it cannot.
        unsafe fn joined(&mut self) -> Option<(u32, u32)> {
            let arena = ARENA_BUFFER.load(Ordering::Relaxed) as Object;
            if arena.is_null() {
                return None;
            }
            self.written();
            let mut key = std::mem::take(&mut self.key);
            key.clear();
            key.push(self.indices as u32);
            key.push(self.kind);
            for d in &self.queue {
                key.push(d.start);
                key.push(d.count);
            }
            let hash = hash(&key);
            let found = self
                .runs
                .get(&hash)
                .filter(|r| *r.key == *key)
                .map(|r| (r.at, r.total, r.low, r.span))
                .filter(|_| crate::config().draw_debug & 1 == 0);
            let run = match found {
                Some(run) => Some(run),
                // SAFETY: the buffers of the run on the drawing thread.
                None => unsafe { self.build(arena, hash, &key) },
            };
            self.key = key;
            let (at, total, low, span) = run?;
            let device = self.device;
            let primitives = if self.kind == TRIANGLE_STRIP {
                total - 2
            } else {
                total / 3
            };
            // SAFETY: the device's own SetIndices and DrawIndexedPrimitive.
            unsafe {
                let set: extern "system" fn(Object, Object) -> i32 =
                    std::mem::transmute(original(104));
                let draw: extern "system" fn(Object, u32, i32, u32, u32, u32, u32) -> i32 =
                    std::mem::transmute(original(82));
                if set(device, arena) < 0 {
                    set(device, self.indices as Object);
                    return self.fail("the module's index buffer cannot be set");
                }
                let drawn = draw(device, self.kind, self.base, low, span, at, primitives);
                set(device, self.indices as Object);
                if drawn < 0 {
                    return self.fail("the joined draw failed");
                }
            }
            Some((at, primitives))
        }

        /// Stitches the indices of the run and writes them into the
        /// module's buffer: where, how many, and the vertices they span.
        ///
        /// # Safety
        /// On the drawing thread; `arena` is the module's buffer and the
        /// index buffer of the run is live.
        unsafe fn build(
            &mut self,
            arena: Object,
            hash: u64,
            key: &[u32],
        ) -> Option<(u32, u32, u32, u32)> {
            let strip = self.kind == TRIANGLE_STRIP;
            let mut out = std::mem::take(&mut self.scratch);
            out.clear();
            let queue = std::mem::take(&mut self.queue);
            let (mut low, mut high) = (u32::MAX, 0u32);
            // SAFETY: the index buffer the game set is live.
            let fits = match unsafe { self.copy(self.indices) } {
                None => false,
                Some(source) => queue.iter().all(|d| {
                    let len = if strip { d.count + 2 } else { d.count * 3 } as usize;
                    let Some(part) = source.get(d.start as usize..d.start as usize + len) else {
                        return false;
                    };
                    if strip {
                        stitch_strip(&mut out, part);
                    } else {
                        out.extend_from_slice(part);
                    }
                    low = low.min(d.min);
                    high = high.max(d.min + d.vertices);
                    true
                }),
            };
            self.queue = queue;
            let total = out.len() as u32;
            // Not an error when it does not fit: a buffer the module does
            // not read (32-bit indices, a dynamic one).
            let result = if !fits || !(3..=ARENA / 4).contains(&total) {
                None
            } else {
                if self.arena_at + total > ARENA {
                    // Full: start again. A plain lock of a managed buffer
                    // may overwrite what earlier draws used.
                    self.arena_at = 0;
                    self.runs.clear();
                }
                let at = self.arena_at;
                // SAFETY: the module's buffer of ARENA 16-bit indices,
                // locked through its own Lock, not the hook.
                let written = unsafe {
                    let lock_at =
                        if *(arena as *const usize) == INDEX_VTABLE.load(Ordering::Relaxed) {
                            INDEX_LOCK.load(Ordering::Relaxed)
                        } else {
                            method(arena, 11)
                        };
                    let lock: LockBuffer = std::mem::transmute(lock_at);
                    let unlock: extern "system" fn(Object) -> i32 =
                        std::mem::transmute(method(arena, 12));
                    let mut data: *mut c_void = std::ptr::null_mut();
                    if lock(arena, at * 2, total * 2, &mut data, 0) < 0 || data.is_null() {
                        false
                    } else {
                        std::ptr::copy_nonoverlapping(out.as_ptr(), data as *mut u16, out.len());
                        unlock(arena);
                        true
                    }
                };
                if written {
                    self.arena_at += total;
                    self.runs.insert(
                        hash,
                        Run {
                            key: key.into(),
                            at,
                            total,
                            low,
                            span: high - low,
                        },
                    );
                    Some((at, total, low, high - low))
                } else {
                    self.fail("the module's index buffer cannot be locked");
                    None
                }
            };
            self.scratch = out;
            result
        }

        /// Makes the method `slot` of the interface of `object` let the
        /// draws held back go first (once per vtable).
        ///
        /// # Safety
        /// `object` must be null or a live COM object with the slot.
        pub unsafe fn watch(&mut self, object: Object, slot: usize) {
            if object.is_null() {
                return;
            }
            // SAFETY: the caller's promise; a vtable is patched once and
            // its own method kept for the thunk.
            unsafe {
                let vtable = *(object as *const *mut usize);
                if self.watched.contains(&(vtable as usize)) {
                    return;
                }
                let n = self.watched.len();
                self.watched.push(vtable as usize);
                if n >= EXTRA {
                    return;
                }
                (*DOCTOR_RENDER_EXTRA.0.get())[n] = *vtable.add(slot);
                patch_table(vtable.add(slot), 1, |at| {
                    *at = doctor_render_extra_thunks as *const () as usize + 16 * n;
                });
            }
        }

        /// [`Merge::watch`] for an index buffer: its `Lock` also tells
        /// which buffers were written.
        ///
        /// # Safety
        /// `buffer` must be null or a live index buffer.
        pub unsafe fn watch_indices(&mut self, buffer: Object) {
            if buffer.is_null() {
                return;
            }
            // SAFETY: the caller's promise.
            unsafe {
                let vtable = *(buffer as *const *mut usize);
                if INDEX_VTABLE.load(Ordering::Relaxed) == 0 {
                    INDEX_LOCK.store(*vtable.add(11), Ordering::Relaxed);
                    INDEX_VTABLE.store(vtable as usize, Ordering::Relaxed);
                    self.watched.push(vtable as usize);
                    patch_table(vtable.add(11), 1, |at| {
                        *at = lock_indices as *const () as usize;
                    });
                } else {
                    // Another class of index buffers: its buffers are not
                    // read, its locks still flush.
                    self.watch(buffer, 11);
                }
            }
        }

        /// [`Merge::watch`] for a texture: its lock and the lock of its
        /// surfaces.
        ///
        /// # Safety
        /// `texture` must be null or a live texture.
        pub unsafe fn watch_texture(&mut self, texture: Object) {
            if texture.is_null() {
                return;
            }
            // SAFETY: the caller's promise; the surface taken is released.
            unsafe {
                if self.watched.contains(&*(texture as *const usize)) {
                    return;
                }
                // LockRect and LockBox are all at slot 19.
                let kind: extern "system" fn(Object) -> u32 =
                    std::mem::transmute(method(texture, 10));
                let kind = kind(texture);
                let mut surface: Object = std::ptr::null_mut();
                match kind {
                    3 => {
                        let level: extern "system" fn(Object, u32, *mut Object) -> i32 =
                            std::mem::transmute(method(texture, 18));
                        level(texture, 0, &mut surface);
                    }
                    5 => {
                        let face: extern "system" fn(Object, u32, u32, *mut Object) -> i32 =
                            std::mem::transmute(method(texture, 18));
                        face(texture, 0, 0, &mut surface);
                    }
                    _ => {}
                }
                self.watch(texture, 19);
                if !surface.is_null() {
                    self.watch(surface, 13);
                    crate::d3d::release(surface);
                }
            }
        }
    }

    /// Stands in `IDirect3DIndexBuffer9::Lock`: a lock for writing makes
    /// the module's copy of the buffer stale; the draws held back go
    /// first.
    unsafe extern "system" fn lock_indices(
        buffer: Object,
        offset: u32,
        size: u32,
        data: *mut *mut c_void,
        flags: u32,
    ) -> i32 {
        if flags & D3DLOCK_READONLY == 0
            && let Ok(mut written) = WRITTEN.lock()
        {
            written.push(buffer as usize);
            WRITTEN_ANY.store(true, Ordering::Relaxed);
        }
        let _ = state();
        // SAFETY: the buffer's own Lock.
        let lock: LockBuffer = unsafe { std::mem::transmute(INDEX_LOCK.load(Ordering::Relaxed)) };
        lock(buffer, offset, size, data, flags)
    }

    /// Makes the module's index buffer, when the hooks go in: once, early
    /// (on the bench a buffer made later, in the first frame of the render
    /// thread while the level loads, made DXVK fail to allocate and fall).
    /// `false`: there is none, and nothing is joined.
    ///
    /// # Safety
    /// The game's device.
    pub unsafe fn prepare(device: Object) -> bool {
        // SAFETY: the device's own CreateIndexBuffer.
        unsafe {
            let create: extern "system" fn(
                Object,
                u32,
                u32,
                u32,
                u32,
                *mut Object,
                *mut c_void,
            ) -> i32 = std::mem::transmute(original(27));
            let mut arena: Object = std::ptr::null_mut();
            let r = create(
                device,
                ARENA * 2,
                D3DUSAGE_WRITEONLY,
                D3DFMT_INDEX16,
                D3DPOOL_MANAGED,
                &mut arena,
                std::ptr::null_mut(),
            );
            if r < 0 || arena.is_null() {
                crate::log(&format!(
                    "draws: the module's index buffer cannot be made ({r:#x}); merge is off"
                ));
                return false;
            }
            ARENA_BUFFER.store(arena as usize, Ordering::Relaxed);
            true
        }
    }
}

/// The hooks: one per method of the device the module stands in.
mod hooks {
    use super::trace::{
        CALLERS, DECLARATION, INDEX_BUFFER, SHADER, SURFACE, TEXTURE, VERTEX_BUFFER,
    };
    use super::*;

    /// The stack pointer inside a hook (for the callers of a draw).
    #[inline(always)]
    fn stack() -> *const u32 {
        let esp: u32;
        // SAFETY: reads a register.
        unsafe {
            core::arch::asm!("mov {}, esp", out(reg) esp, options(nostack, nomem, preserves_flags));
        }
        esp as usize as *const u32
    }

    /// Counts a draw of the game.
    #[inline(always)]
    fn drawn(s: &mut State, primitives: u32) {
        s.counters.calls += 1;
        s.counters.draws += 1;
        s.counters.primitives += u64::from(primitives);
        if s.trace.on {
            let mut found = [0u32; 8];
            // SAFETY: the stack of this thread.
            let n = unsafe { callers(stack(), &mut found) };
            s.trace.record(CALLERS, &found[..n]);
        }
    }

    macro_rules! call {
        ($slot:expr, ($($ty:ty),*), ($($arg:expr),*)) => {{
            // SAFETY: the device's own method of this slot, stdcall.
            let f: extern "system" fn(Object $(, $ty)*) -> i32 =
                unsafe { std::mem::transmute(original($slot)) };
            f($($arg),*)
        }};
    }

    /// A hook that only counts and records its arguments.
    macro_rules! plain {
        ($name:ident, $slot:expr, ($($arg:ident: $ty:ty),*)) => {
            unsafe extern "system" fn $name(device: Object $(, $arg: $ty)*) -> i32 {
                if let Some(s) = state() {
                    s.counters.calls += 1;
                    if s.trace.on {
                        s.trace.record($slot, &[$($arg as usize as u32),*]);
                    }
                }
                call!($slot, ($($ty),*), (device $(, $arg)*))
            }
        };
    }

    plain!(begin_scene, 41, ());
    plain!(end_scene, 42, ());
    plain!(set_stream_freq, 102, (stream: u32, setting: u32));
    plain!(stretch_rect, 34, (source: Object, source_rect: *const c_void, target: Object, target_rect: *const c_void, filter: u32));

    unsafe extern "system" fn set_render_target(
        device: Object,
        index: u32,
        surface: Object,
    ) -> i32 {
        if let Some(s) = state() {
            s.counters.calls += 1;
            if s.trace.on {
                // SAFETY: the surface the game sets is live.
                unsafe { s.trace.describe(SURFACE, surface) };
                s.trace.record(37, &[index, surface as usize as u32]);
            }
            if s.want_merge {
                // SAFETY: as above.
                unsafe { s.merge.watch(surface, 13) };
            }
        }
        call!(37, (u32, Object), (device, index, surface))
    }

    unsafe extern "system" fn set_depth_stencil(device: Object, surface: Object) -> i32 {
        if let Some(s) = state() {
            s.counters.calls += 1;
            if s.trace.on {
                // SAFETY: the surface the game sets is live.
                unsafe { s.trace.describe(SURFACE, surface) };
                s.trace.record(39, &[surface as usize as u32]);
            }
            if s.want_merge {
                // SAFETY: as above.
                unsafe { s.merge.watch(surface, 13) };
            }
        }
        call!(39, (Object), (device, surface))
    }

    unsafe extern "system" fn clear(
        device: Object,
        count: u32,
        rects: *const c_void,
        flags: u32,
        colour: u32,
        z: f32,
        stencil: u32,
    ) -> i32 {
        if let Some(s) = state() {
            s.counters.calls += 1;
            if s.trace.on {
                s.trace
                    .record(43, &[count, flags, colour, z.to_bits(), stencil]);
            }
        }
        call!(
            43,
            (u32, *const c_void, u32, u32, f32, u32),
            (device, count, rects, flags, colour, z, stencil)
        )
    }

    unsafe extern "system" fn set_viewport(device: Object, viewport: *const u32) -> i32 {
        if let Some(s) = state() {
            s.counters.calls += 1;
            if s.trace.on {
                // SAFETY: a D3DVIEWPORT9 of six words.
                unsafe { s.trace.record_data(47, &[], viewport, 6) };
            }
        }
        call!(47, (*const u32), (device, viewport))
    }

    unsafe extern "system" fn set_scissor(device: Object, rect: *const u32) -> i32 {
        if let Some(s) = state() {
            s.counters.calls += 1;
            if s.trace.on {
                // SAFETY: a RECT of four words.
                unsafe { s.trace.record_data(75, &[], rect, 4) };
            }
        }
        call!(75, (*const u32), (device, rect))
    }

    unsafe extern "system" fn set_render_state(device: Object, state_: u32, value: u32) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.render_state(state_, value);
            let dropped = s.setter(same);
            if s.trace.on {
                s.trace.record(57, &[state_, value, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
        }
        call!(57, (u32, u32), (device, state_, value))
    }

    unsafe extern "system" fn set_texture(device: Object, sampler: u32, texture: Object) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.texture(sampler, texture as usize);
            let dropped = s.setter(same);
            if s.trace.on {
                // SAFETY: the texture the game sets is live.
                unsafe { s.trace.describe(TEXTURE, texture) };
                s.trace
                    .record(65, &[sampler, texture as usize as u32, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
            if s.want_merge {
                // SAFETY: as above.
                unsafe { s.merge.watch_texture(texture) };
            }
        }
        call!(65, (u32, Object), (device, sampler, texture))
    }

    unsafe extern "system" fn set_sampler_state(
        device: Object,
        sampler: u32,
        state_: u32,
        value: u32,
    ) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.sampler_state(sampler, state_, value);
            let dropped = s.setter(same);
            if s.trace.on {
                s.trace
                    .record(69, &[sampler, state_, value, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
        }
        call!(69, (u32, u32, u32), (device, sampler, state_, value))
    }

    unsafe extern "system" fn draw_primitive(
        device: Object,
        kind: u32,
        start: u32,
        count: u32,
    ) -> i32 {
        if let Some(s) = state() {
            drawn(s, count);
            s.counters.draws_out += 1;
            if s.trace.on {
                s.trace.record(81, &[kind, start, count]);
            }
        }
        call!(81, (u32, u32, u32), (device, kind, start, count))
    }

    unsafe extern "system" fn draw_indexed(
        device: Object,
        kind: u32,
        base: i32,
        min: u32,
        vertices: u32,
        start: u32,
        count: u32,
    ) -> i32 {
        if let Some(s) = state_raw() {
            drawn(s, count);
            if s.trace.on {
                s.trace
                    .record(82, &[kind, base as u32, min, vertices, start, count]);
            }
            let indices = s.filter.current_indices();
            // SAFETY: on the drawing thread, the game's device.
            if unsafe {
                s.merge.hold(
                    &mut s.counters,
                    &mut s.trace,
                    device,
                    indices,
                    kind,
                    base,
                    [min, vertices, start, count],
                )
            } {
                return 0;
            }
            s.flush();
            s.counters.draws_out += 1;
        }
        call!(
            82,
            (u32, i32, u32, u32, u32, u32),
            (device, kind, base, min, vertices, start, count)
        )
    }

    unsafe extern "system" fn draw_up(
        device: Object,
        kind: u32,
        count: u32,
        data: *const c_void,
        stride: u32,
    ) -> i32 {
        if let Some(s) = state() {
            drawn(s, count);
            s.counters.draws_out += 1;
            // Stream 0 is unset after a draw from user memory.
            s.filter.stream(0, 0, 0, 0);
            if s.trace.on {
                s.trace.record(83, &[kind, count, stride]);
            }
        }
        call!(
            83,
            (u32, u32, *const c_void, u32),
            (device, kind, count, data, stride)
        )
    }

    unsafe extern "system" fn draw_indexed_up(
        device: Object,
        kind: u32,
        min: u32,
        vertices: u32,
        count: u32,
        index_data: *const c_void,
        index_format: u32,
        data: *const c_void,
        stride: u32,
    ) -> i32 {
        if let Some(s) = state() {
            drawn(s, count);
            s.counters.draws_out += 1;
            // Stream 0 and the indices are unset after it.
            s.filter.stream(0, 0, 0, 0);
            s.filter.indices(0);
            if s.trace.on {
                s.trace
                    .record(84, &[kind, min, vertices, count, index_format, stride]);
            }
        }
        call!(
            84,
            (u32, u32, u32, u32, *const c_void, u32, *const c_void, u32),
            (
                device,
                kind,
                min,
                vertices,
                count,
                index_data,
                index_format,
                data,
                stride
            )
        )
    }

    unsafe extern "system" fn set_declaration(device: Object, declaration: Object) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.declaration(declaration as usize);
            let dropped = s.setter(same);
            if s.trace.on {
                // SAFETY: the declaration the game sets is live.
                unsafe { s.trace.describe(DECLARATION, declaration) };
                s.trace
                    .record(87, &[declaration as usize as u32, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
        }
        call!(87, (Object), (device, declaration))
    }

    unsafe extern "system" fn set_vertex_shader(device: Object, shader: Object) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.vertex_shader(shader as usize);
            let dropped = s.setter(same);
            if s.trace.on {
                // SAFETY: the shader the game sets is live.
                unsafe { s.trace.describe(SHADER, shader) };
                s.trace
                    .record(92, &[shader as usize as u32, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
        }
        call!(92, (Object), (device, shader))
    }

    unsafe extern "system" fn set_pixel_shader(device: Object, shader: Object) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.pixel_shader(shader as usize);
            let dropped = s.setter(same);
            if s.trace.on {
                // SAFETY: the shader the game sets is live.
                unsafe { s.trace.describe(SHADER, shader) };
                s.trace
                    .record(107, &[shader as usize as u32, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
        }
        call!(107, (Object), (device, shader))
    }

    unsafe extern "system" fn set_vs_constants(
        device: Object,
        start: u32,
        data: *const [u32; 4],
        count: u32,
    ) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            // SAFETY: the game passes `count` vectors.
            let same = unsafe { s.filter.vs_constants(start, data, count) };
            let dropped = s.setter(same);
            if s.trace.on {
                let n = (count as usize).min(256) * 4;
                // SAFETY: as above.
                unsafe {
                    s.trace
                        .record_data(94, &[start, count, u32::from(same)], data as *const u32, n)
                };
            }
            if dropped {
                return 0;
            }
        }
        call!(
            94,
            (u32, *const [u32; 4], u32),
            (device, start, data, count)
        )
    }

    unsafe extern "system" fn set_ps_constants(
        device: Object,
        start: u32,
        data: *const [u32; 4],
        count: u32,
    ) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            // SAFETY: the game passes `count` vectors.
            let same = unsafe { s.filter.ps_constants(start, data, count) };
            let dropped = s.setter(same);
            if s.trace.on {
                let n = (count as usize).min(256) * 4;
                // SAFETY: as above.
                unsafe {
                    s.trace.record_data(
                        109,
                        &[start, count, u32::from(same)],
                        data as *const u32,
                        n,
                    )
                };
            }
            if dropped {
                return 0;
            }
        }
        call!(
            109,
            (u32, *const [u32; 4], u32),
            (device, start, data, count)
        )
    }

    unsafe extern "system" fn set_stream(
        device: Object,
        index: u32,
        buffer: Object,
        offset: u32,
        stride: u32,
    ) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.stream(index, buffer as usize, offset, stride);
            let dropped = s.setter(same);
            if s.trace.on {
                // SAFETY: the buffer the game sets is live.
                unsafe { s.trace.describe(VERTEX_BUFFER, buffer) };
                s.trace.record(
                    100,
                    &[
                        index,
                        buffer as usize as u32,
                        offset,
                        stride,
                        u32::from(same),
                    ],
                );
            }
            if dropped {
                return 0;
            }
            if s.want_merge {
                // SAFETY: as above; `Lock` of a vertex buffer is slot 11.
                unsafe { s.merge.watch(buffer, 11) };
            }
        }
        call!(
            100,
            (u32, Object, u32, u32),
            (device, index, buffer, offset, stride)
        )
    }

    unsafe extern "system" fn set_indices(device: Object, buffer: Object) -> i32 {
        if let Some(s) = state_raw()
            && s.busy()
        {
            let same = s.filter.indices(buffer as usize);
            let dropped = s.setter(same);
            if s.trace.on {
                // SAFETY: the buffer the game sets is live.
                unsafe { s.trace.describe(INDEX_BUFFER, buffer) };
                s.trace
                    .record(104, &[buffer as usize as u32, u32::from(same)]);
            }
            if dropped {
                return 0;
            }
            if s.want_merge {
                // SAFETY: as above.
                unsafe { s.merge.watch_indices(buffer) };
            }
        }
        call!(104, (Object), (device, buffer))
    }

    unsafe extern "system" fn set_fvf(device: Object, fvf: u32) -> i32 {
        if let Some(s) = state() {
            s.counters.calls += 1;
            // It replaces the vertex declaration.
            s.filter.forget_declaration();
            if s.trace.on {
                s.trace.record(89, &[fvf]);
            }
        }
        call!(89, (u32), (device, fvf))
    }

    /// A state block of the game would set the state behind the hooks:
    /// filter and merge stop for good (the module's own block tells the
    /// filter when it is applied).
    unsafe extern "system" fn create_state_block(
        device: Object,
        kind: u32,
        out: *mut Object,
    ) -> i32 {
        if let Some(s) = state()
            && !OWN_STATE_BLOCK.load(Ordering::Relaxed)
        {
            s.filter.poison("the game made a state block");
            s.merge.failed = true;
            s.merge.on = false;
        }
        call!(59, (u32, *mut Object), (device, kind, out))
    }

    unsafe extern "system" fn begin_state_block(device: Object) -> i32 {
        if let Some(s) = state() {
            s.filter.poison("the game recorded a state block");
            s.merge.failed = true;
            s.merge.on = false;
        }
        call!(60, (), (device))
    }

    /// A query brackets draws: its `Issue` lets the draws held back go
    /// first.
    unsafe extern "system" fn create_query(device: Object, kind: u32, out: *mut Object) -> i32 {
        let r = call!(118, (u32, *mut Object), (device, kind, out));
        if r >= 0
            && !out.is_null()
            && let Some(s) = state()
            && s.want_merge
        {
            // SAFETY: the query just made; `Issue` is slot 6.
            unsafe { s.merge.watch(*out, 6) };
        }
        r
    }

    /// `Reset` sets the state behind the hooks and wants every resource of
    /// the `DEFAULT` pool gone.
    unsafe extern "system" fn reset(device: Object, parameters: *mut c_void) -> i32 {
        let _ = state();
        super::forget();
        let r = call!(16, (*mut c_void), (device, parameters));
        super::forget();
        r
    }

    pub fn table() -> Vec<(usize, usize)> {
        vec![
            (16, reset as *const () as usize),
            (34, stretch_rect as *const () as usize),
            (37, set_render_target as *const () as usize),
            (39, set_depth_stencil as *const () as usize),
            (41, begin_scene as *const () as usize),
            (42, end_scene as *const () as usize),
            (43, clear as *const () as usize),
            (47, set_viewport as *const () as usize),
            (57, set_render_state as *const () as usize),
            (59, create_state_block as *const () as usize),
            (60, begin_state_block as *const () as usize),
            (65, set_texture as *const () as usize),
            (69, set_sampler_state as *const () as usize),
            (75, set_scissor as *const () as usize),
            (81, draw_primitive as *const () as usize),
            (82, draw_indexed as *const () as usize),
            (83, draw_up as *const () as usize),
            (84, draw_indexed_up as *const () as usize),
            (87, set_declaration as *const () as usize),
            (89, set_fvf as *const () as usize),
            (92, set_vertex_shader as *const () as usize),
            (94, set_vs_constants as *const () as usize),
            (100, set_stream as *const () as usize),
            (102, set_stream_freq as *const () as usize),
            (104, set_indices as *const () as usize),
            (107, set_pixel_shader as *const () as usize),
            (109, set_ps_constants as *const () as usize),
            (118, create_query as *const () as usize),
        ]
    }
}
