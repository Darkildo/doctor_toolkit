//! The profile of the game measured from inside (stage 58,
//! [`doctor_toolkit_live::profile`]).
//!
//! From `DllMain` on the module counts, without a lock and without
//! allocating in the hooks:
//!
//! - every pass of the main loop ([`pass`], from the frame hook): the time
//!   from one pass to the next;
//! - every present of the render thread: the call at `0x0040DFA2` goes to
//!   [`present`], which takes the performance counter before and after
//!   `0x0060D8B0`;
//! - every `ReadFile` of the game through its import slot `0x00D55268`
//!   (the calls patched like those of `CreateFileA`, stage 46): reads,
//!   bytes and time by thread and by call, and a ring of slow or large
//!   reads.
//!
//! [`answer`] copies the counters on the connection's thread, reads the
//! CPU time of the threads, the texture table, the clock values of the game
//! and, when asked, walks the address space and samples where the main
//! and render threads stand.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use doctor_toolkit_live::addr::{
    BACK_BUFFER_HEIGHT, BACK_BUFFER_WIDTH, FIXED_STEP, GAME_FRAME, IAT_READ_FILE, PRESENT_CALL,
    PRESENT_CALLEE, READ_FILE_CALLS, READ_FILE_LOADS, SIM_HZ, SIM_TIME_MS, TEXTURE_TABLE, VSYNC,
    WINDOWED_LIMITER,
};
use doctor_toolkit_live::profile::{
    Clock, CpuTime, IMAGE_CODE, IO_THREADS, LARGE_READ, MAX_SAMPLE_MS, MemoryMap, Profile, RING,
    SLOW_READ_US, SLOW_RING, SiteIo, SlowRead, TEXTURES, TOP_EIPS, TextureUse, ThreadIo,
    ThreadSamples, in_image,
};
use doctor_toolkit_live::{ErrorCode, Response};

use crate::win::{self, Handle};

// --- Clock ------------------------------------------------------------------

static FREQUENCY: AtomicU64 = AtomicU64::new(0);
static START: AtomicU64 = AtomicU64::new(0);
static RESET_AT: AtomicU64 = AtomicU64::new(0);

fn qpc() -> u64 {
    let mut v = 0i64;
    // SAFETY: fills `v`.
    unsafe { win::QueryPerformanceCounter(&mut v) };
    v as u64
}

/// Microseconds of `ticks` of the performance counter (no overflow over
/// days of uptime).
fn micros(ticks: u64) -> u64 {
    let f = FREQUENCY.load(Relaxed);
    if f == 0 {
        return 0;
    }
    ticks / f * 1_000_000 + ticks % f * 1_000_000 / f
}

fn clamp(v: u64) -> u32 {
    v.min(u64::from(u32::MAX)) as u32
}

/// Microseconds since the module started.
fn uptime_us() -> u64 {
    micros(qpc().saturating_sub(START.load(Relaxed)))
}

// --- Rings ------------------------------------------------------------------

/// The last [`RING`] durations, one writer (the main or the render
/// thread), read by the connection threads.
struct Ring {
    /// Entries written since the reset.
    n: AtomicU32,
    slots: [AtomicU32; RING],
}

impl Ring {
    const fn new() -> Self {
        Self {
            n: AtomicU32::new(0),
            slots: [const { AtomicU32::new(0) }; RING],
        }
    }

    fn push(&self, v: u32) {
        let i = self.n.fetch_add(1, Relaxed);
        self.slots[i as usize % RING].store(v, Relaxed);
    }

    /// Oldest first.
    fn read(&self) -> Vec<u32> {
        let n = self.n.load(Relaxed);
        let k = n.min(RING as u32);
        (n - k..n)
            .map(|i| self.slots[i as usize % RING].load(Relaxed))
            .collect()
    }

    fn reset(&self) {
        self.n.store(0, Relaxed);
    }
}

// --- Main loop and presents ---------------------------------------------------

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static RENDER_THREAD: AtomicU32 = AtomicU32::new(0);
static PASSES: AtomicU64 = AtomicU64::new(0);
static LAST_PASS: AtomicU64 = AtomicU64::new(0);
static PASS_RING: Ring = Ring::new();
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static LAST_PRESENT: AtomicU64 = AtomicU64::new(0);
static INTERVAL_RING: Ring = Ring::new();
static PRESENT_RING: Ring = Ring::new();
static PRESENT_HOOKED: AtomicBool = AtomicBool::new(false);
static READS_HOOKED: AtomicBool = AtomicBool::new(false);

/// One pass of the main loop (from the frame hook, first thing).
pub fn pass() {
    let now = qpc();
    // SAFETY: a plain Win32 call.
    MAIN_THREAD.store(unsafe { win::GetCurrentThreadId() }, Relaxed);
    let last = LAST_PASS.swap(now, Relaxed);
    if last != 0 {
        PASS_RING.push(clamp(micros(now.wrapping_sub(last))));
    }
    PASSES.fetch_add(1, Relaxed);
}

/// Stands at `0x0040DFA2` for `0x0060D8B0` (`cdecl`, two arguments; the
/// caller pops them and ignores the result).
extern "C" fn present(a: u32, b: u32) -> u32 {
    let t0 = qpc();
    // SAFETY: the function the patched call reached (checked bytes).
    let original: extern "C" fn(u32, u32) -> u32 =
        unsafe { std::mem::transmute(PRESENT_CALLEE as usize) };
    let r = original(a, b);
    let t1 = qpc();
    // SAFETY: a plain Win32 call.
    RENDER_THREAD.store(unsafe { win::GetCurrentThreadId() }, Relaxed);
    let last = LAST_PRESENT.swap(t0, Relaxed);
    if last != 0 {
        INTERVAL_RING.push(clamp(micros(t0.wrapping_sub(last))));
    }
    PRESENT_RING.push(clamp(micros(t1.wrapping_sub(t0))));
    PRESENTS.fetch_add(1, Relaxed);
    r
}

// --- Reads ------------------------------------------------------------------

struct Io {
    thread: AtomicU32,
    reads: AtomicU32,
    bytes: AtomicU64,
    us: AtomicU64,
    max: AtomicU32,
    slow: AtomicU32,
}

impl Io {
    const fn new() -> Self {
        Self {
            thread: AtomicU32::new(0),
            reads: AtomicU32::new(0),
            bytes: AtomicU64::new(0),
            us: AtomicU64::new(0),
            max: AtomicU32::new(0),
            slow: AtomicU32::new(0),
        }
    }

    fn add(&self, bytes: u32, us: u32) {
        self.reads.fetch_add(1, Relaxed);
        self.bytes.fetch_add(u64::from(bytes), Relaxed);
        self.us.fetch_add(u64::from(us), Relaxed);
        self.max.fetch_max(us, Relaxed);
        if us >= SLOW_READ_US {
            self.slow.fetch_add(1, Relaxed);
        }
    }

    fn reset(&self) {
        self.reads.store(0, Relaxed);
        self.bytes.store(0, Relaxed);
        self.us.store(0, Relaxed);
        self.max.store(0, Relaxed);
        self.slow.store(0, Relaxed);
    }

    fn get(&self, thread: u32) -> ThreadIo {
        ThreadIo {
            thread,
            reads: self.reads.load(Relaxed),
            bytes: self.bytes.load(Relaxed),
            total_us: self.us.load(Relaxed),
            max_us: self.max.load(Relaxed),
            slow: self.slow.load(Relaxed),
        }
    }
}

/// The first [`IO_THREADS`] threads that read, then the others together.
static THREADS: [Io; IO_THREADS + 1] = [const { Io::new() }; IO_THREADS + 1];

/// The patched instructions: the calls, then the loads into `edi`.
const SITES: [u32; 8] = [
    READ_FILE_CALLS[0],
    READ_FILE_CALLS[1],
    READ_FILE_CALLS[2],
    READ_FILE_CALLS[3],
    READ_FILE_CALLS[4],
    READ_FILE_CALLS[5],
    READ_FILE_CALLS[6],
    READ_FILE_LOADS[0],
];
static SITE_IO: [Io; SITES.len()] = [const { Io::new() }; SITES.len()];

struct Slow {
    thread: AtomicU32,
    bytes: AtomicU32,
    us: AtomicU32,
    at_ms: AtomicU32,
}

static SLOW: [Slow; SLOW_RING] = [const {
    Slow {
        thread: AtomicU32::new(0),
        bytes: AtomicU32::new(0),
        us: AtomicU32::new(0),
        at_ms: AtomicU32::new(0),
    }
}; SLOW_RING];
static SLOW_N: AtomicU32 = AtomicU32::new(0);

fn thread_io(thread: u32) -> &'static Io {
    for io in &THREADS[..IO_THREADS] {
        let t = io.thread.load(Relaxed);
        if t == thread {
            return io;
        }
        if t == 0 {
            match io.thread.compare_exchange(0, thread, Relaxed, Relaxed) {
                Ok(_) => return io,
                Err(t) if t == thread => return io,
                Err(_) => {}
            }
        }
    }
    &THREADS[IO_THREADS]
}

fn record_read(site: usize, bytes: u32, ticks: u64, end: u64) {
    let us = clamp(micros(ticks));
    // SAFETY: a plain Win32 call.
    let thread = unsafe { win::GetCurrentThreadId() };
    thread_io(thread).add(bytes, us);
    SITE_IO[site].add(bytes, us);
    if bytes >= LARGE_READ || us >= SLOW_READ_US {
        let i = SLOW_N.fetch_add(1, Relaxed) as usize % SLOW_RING;
        let s = &SLOW[i];
        s.thread.store(thread, Relaxed);
        s.bytes.store(bytes, Relaxed);
        s.us.store(us, Relaxed);
        s.at_ms.store(
            clamp(micros(end.saturating_sub(START.load(Relaxed))) / 1000),
            Relaxed,
        );
    }
}

type ReadFile = extern "system" fn(Handle, *mut u8, u32, *mut u32, *mut u8) -> i32;

/// What the import slot holds now: `ReadFile` of `kernel32`, or the hook of
/// another mod with the same arguments.
fn read_file_slot() -> ReadFile {
    // SAFETY: an import slot checked by `edition::check`, a 4-byte aligned
    // pointer the loader filled; it holds a function of these arguments.
    unsafe {
        let f = std::ptr::read_volatile(IAT_READ_FILE as usize as *const u32) as usize;
        std::mem::transmute::<usize, ReadFile>(f)
    }
}

/// One instance per patched instruction (`SITE`: its index in [`SITES`]).
extern "system" fn read_file<const SITE: usize>(
    file: Handle,
    buffer: *mut u8,
    len: u32,
    read: *mut u32,
    overlapped: *mut u8,
) -> i32 {
    let f = read_file_slot();
    let t0 = qpc();
    // A stream composed from a resource patch (stage 100) is answered by
    // the module; every other handle by what the slot holds.
    // SAFETY: the arguments of the game's call.
    let ok = match unsafe { crate::resources::read(file, buffer, len, read, overlapped) } {
        Some(ok) => ok,
        None => f(file, buffer, len, read, overlapped),
    };
    let t1 = qpc();
    // SAFETY: plain Win32 calls; the game reads the error of the read.
    let error = unsafe { win::GetLastError() };
    let bytes = if read.is_null() {
        len
    } else {
        // SAFETY: the game's out-parameter of the read, filled by it.
        unsafe { std::ptr::read_volatile(read) }
    };
    record_read(SITE, bytes, t1.wrapping_sub(t0), t1);
    // SAFETY: as above.
    unsafe { win::SetLastError(error) };
    ok
}

const READ_HOOKS: [ReadFile; SITES.len()] = [
    read_file::<0>,
    read_file::<1>,
    read_file::<2>,
    read_file::<3>,
    read_file::<4>,
    read_file::<5>,
    read_file::<6>,
    read_file::<7>,
];

// --- Installation ---------------------------------------------------------

/// Starts the clock (first thing of the installation, before the frame
/// hook can run).
pub fn start_clock() {
    let mut f = 0i64;
    // SAFETY: fills `f`.
    unsafe { win::QueryPerformanceFrequency(&mut f) };
    FREQUENCY.store(f.max(0) as u64, Relaxed);
    START.store(qpc(), Relaxed);
}

/// Patches the present and the reads (after the edition check, from
/// `DllMain`). Returns the instructions it could not patch.
pub fn install() -> Vec<u32> {
    let mut failed = Vec::new();
    let hook = present as extern "C" fn(u32, u32) -> u32 as usize as u32;
    let rel = hook.wrapping_sub(PRESENT_CALL + 5);
    // SAFETY: the call checked by `edition::check`; from DllMain, before
    // the render thread runs. Only its 4-byte target changes.
    if unsafe { win::patch(PRESENT_CALL + 1, &rel.to_le_bytes()) } {
        PRESENT_HOOKED.store(true, Relaxed);
    } else {
        failed.push(PRESENT_CALL);
    }
    let mut reads = true;
    for (i, (&at, &hook)) in SITES.iter().zip(READ_HOOKS.iter()).enumerate() {
        let hook = hook as usize as u32;
        let mut code = [0u8, 0, 0, 0, 0, 0x90];
        if i < READ_FILE_CALLS.len() {
            // `call [slot]` -> `call hook; nop`.
            code[0] = 0xE8;
            code[1..5].copy_from_slice(&hook.wrapping_sub(at + 5).to_le_bytes());
        } else {
            // `mov edi, [slot]` -> `mov edi, hook; nop`.
            code[0] = 0xBF;
            code[1..5].copy_from_slice(&hook.to_le_bytes());
        }
        // SAFETY: an instruction checked by `edition::check`; from
        // DllMain, before the game's threads run.
        if !unsafe { win::patch(at, &code) } {
            failed.push(at);
            reads = false;
        }
    }
    READS_HOOKED.store(reads, Relaxed);
    failed
}

/// One line for the log after the installation.
pub fn summary() -> String {
    format!(
        "profile: present at 0x0040DFA2 {}, {} calls of ReadFile (0x00D55268) {}, counter {} Hz",
        if PRESENT_HOOKED.load(Relaxed) {
            "on"
        } else {
            "off"
        },
        SITES.len(),
        if READS_HOOKED.load(Relaxed) {
            "on"
        } else {
            "off"
        },
        FREQUENCY.load(Relaxed)
    )
}

// --- Threads ----------------------------------------------------------------

/// Handles of the threads opened once: thread id, handle, whether it may
/// be suspended and its context read.
static HANDLES: Mutex<Vec<(u32, usize, bool)>> = Mutex::new(Vec::new());

fn thread_handle(thread: u32) -> Option<(usize, bool)> {
    if thread == 0 {
        return None;
    }
    let mut list = HANDLES.lock().ok()?;
    if let Some(&(_, h, sample)) = list.iter().find(|e| e.0 == thread) {
        return (h != 0).then_some((h, sample));
    }
    // SAFETY: plain Win32 calls; the handle is kept for the module's life.
    let (h, sample) = unsafe {
        let h = win::OpenThread(win::THREAD_SAMPLE, 0, thread);
        if h.is_null() {
            (
                win::OpenThread(win::THREAD_QUERY_INFORMATION, 0, thread),
                false,
            )
        } else {
            (h, true)
        }
    };
    let h = h as usize;
    list.push((thread, h, sample));
    (h != 0).then_some((h, sample))
}

fn ms_of(filetime: u64) -> u32 {
    clamp(filetime / 10_000)
}

fn thread_cpu(thread: u32) -> Option<CpuTime> {
    let (h, _) = thread_handle(thread)?;
    let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: an open handle; fills the four times.
    let ok = unsafe { win::GetThreadTimes(h as Handle, &mut c, &mut e, &mut k, &mut u) };
    (ok != 0).then(|| CpuTime {
        user_ms: ms_of(u),
        kernel_ms: ms_of(k),
    })
}

fn process_cpu() -> Option<CpuTime> {
    let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: the pseudo handle of this process; fills the four times.
    let ok =
        unsafe { win::GetProcessTimes(win::GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    (ok != 0).then(|| CpuTime {
        user_ms: ms_of(u),
        kernel_ms: ms_of(k),
    })
}

// --- Sampler ------------------------------------------------------------------

/// Slots of the table of instruction pointers of one thread.
const TABLE: usize = 16_384;
static SAMPLING: AtomicBool = AtomicBool::new(false);

struct Sampled {
    thread: u32,
    handle: usize,
    /// Highest address of the thread's stack (`NT_TIB.StackBase`), 0 when
    /// unknown: the stack is then not read.
    stack_base: u32,
    /// `(eip, count)`; eip 0 — an empty slot.
    table: Vec<(u32, u32)>,
    /// `(return address into the game, count)` of samples outside it.
    callers: Vec<(u32, u32)>,
    samples: u32,
    failed: u32,
    dropped: u32,
    distinct: u32,
}

/// Counts `key` in an open-addressing table of [`TABLE`] slots; false when
/// it is full.
fn count_in(table: &mut [(u32, u32)], key: u32) -> Option<bool> {
    let mask = TABLE - 1;
    let mut i = (key.wrapping_mul(0x9E37_79B1) >> 18) as usize & mask;
    for _ in 0..TABLE {
        let slot = &mut table[i];
        if slot.0 == key {
            slot.1 += 1;
            return Some(false);
        }
        if slot.0 == 0 {
            *slot = (key, 1);
            return Some(true);
        }
        i = (i + 1) & mask;
    }
    None
}

/// Whether `ret` returns into the game's code right after a call: `E8
/// rel32`, `FF 15 [mem]`, or `FF /2` through a register (`FF D0+r`), a
/// pointer in a register (`FF 10+r`), with a byte or a word of offset
/// (`FF 50+r d8`, `FF 90+r d32`) or off the stack (`FF 54 24 d8`, `FF 94
/// 24 d32`) — the calls of the game's compiler, virtual ones included.
fn returns_into_image(ret: u32) -> bool {
    if !(IMAGE_CODE.0 + 7..=IMAGE_CODE.1).contains(&ret) {
        return false;
    }
    // SAFETY: the game's code section is mapped for the life of the process.
    let b = |back: u32| unsafe { std::ptr::read_volatile((ret - back) as usize as *const u8) };
    b(5) == 0xE8
        || (b(6) == 0xFF && b(5) == 0x15)
        || (b(2) == 0xFF && b(1) & 0xF8 == 0xD0)
        || (b(2) == 0xFF && b(1) & 0xF8 == 0x10 && b(1) & 7 != 4 && b(1) & 7 != 5)
        || (b(3) == 0xFF && b(2) & 0xF8 == 0x50 && b(2) & 7 != 4)
        || (b(6) == 0xFF && b(5) & 0xF8 == 0x90 && b(5) & 7 != 4)
        || (b(4) == 0xFF && b(3) == 0x54 && b(2) == 0x24)
        || (b(7) == 0xFF && b(6) == 0x94 && b(5) == 0x24)
}

/// Words of the stack read for the caller of a sample outside the game.
const STACK_WORDS: u32 = 512;

impl Sampled {
    fn insert(&mut self, eip: u32) {
        if eip == 0 {
            self.dropped += 1;
            return;
        }
        match count_in(&mut self.table, eip) {
            Some(new) => {
                self.samples += 1;
                self.distinct += u32::from(new);
            }
            None => self.dropped += 1,
        }
    }

    /// The first return address into the game on the stack from `esp` up;
    /// the thread is suspended. Reads only the committed part between
    /// `esp` and the stack's top.
    fn caller(&self, esp: u32) -> Option<u32> {
        let base = self.stack_base;
        if base == 0 || esp >= base || base - esp > 64 << 20 || esp & 3 != 0 {
            return None;
        }
        let end = base.min(esp.saturating_add(STACK_WORDS * 4));
        let mut at = esp;
        while at + 4 <= end {
            // SAFETY: between the stack pointer and the top of the stack of
            // a thread of this process: committed memory.
            let word = unsafe { std::ptr::read_volatile(at as usize as *const u32) };
            if returns_into_image(word) {
                return Some(word);
            }
            at += 4;
        }
        None
    }

    fn finish(self, elapsed_ms: u32) -> ThreadSamples {
        let top_of = |table: Vec<(u32, u32)>| {
            let mut top: Vec<(u32, u32)> = table.into_iter().filter(|e| e.0 != 0).collect();
            top.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            top.truncate(TOP_EIPS);
            top
        };
        ThreadSamples {
            thread: self.thread,
            samples: self.samples,
            failed: self.failed,
            dropped: self.dropped,
            distinct: self.distinct,
            elapsed_ms,
            top: top_of(self.table),
            callers: top_of(self.callers),
        }
    }
}

/// `NT_TIB.StackBase` of a thread: from its TEB (`ThreadBasicInformation`).
fn stack_base(handle: usize) -> u32 {
    let mut info = [0u32; 7];
    // SAFETY: a plain call with a buffer of the size of the class.
    let status = unsafe {
        win::NtQueryInformationThread(
            handle as Handle,
            0,
            info.as_mut_ptr().cast(),
            28,
            std::ptr::null_mut(),
        )
    };
    let teb = info[1];
    if status != 0 || teb == 0 {
        return 0;
    }
    // SAFETY: the TEB of a live thread of this process is mapped.
    unsafe { std::ptr::read_volatile((teb + 4) as usize as *const u32) }
}

/// Every ~1 ms suspends each thread, reads its instruction pointer and
/// resumes it. Nothing is allocated, locked or logged while a thread is
/// suspended: the tables and the context are made before.
fn sample(ms: u32) -> Result<Vec<ThreadSamples>, String> {
    if SAMPLING.swap(true, Relaxed) {
        return Err("another client samples the threads now".into());
    }
    // SAFETY: a plain Win32 call.
    let me = unsafe { win::GetCurrentThreadId() };
    let mut threads: Vec<Sampled> = Vec::new();
    for thread in [MAIN_THREAD.load(Relaxed), RENDER_THREAD.load(Relaxed)] {
        if thread == 0 || thread == me || threads.iter().any(|t| t.thread == thread) {
            continue;
        }
        if let Some((handle, true)) = thread_handle(thread) {
            threads.push(Sampled {
                thread,
                handle,
                stack_base: stack_base(handle),
                table: vec![(0, 0); TABLE],
                callers: vec![(0, 0); TABLE],
                samples: 0,
                failed: 0,
                dropped: 0,
                distinct: 0,
            });
        }
    }
    let mut context = Box::new(win::Context { words: [0; 179] });
    let start = Instant::now();
    let end = start + Duration::from_millis(u64::from(ms.min(MAX_SAMPLE_MS)));
    while !threads.is_empty() && Instant::now() < end {
        for t in threads.iter_mut() {
            let h = t.handle as Handle;
            context.words[win::Context::FLAGS] = win::CONTEXT_CONTROL;
            // SAFETY: an open handle with the rights to suspend and read
            // the context; the thread is resumed right after, and nothing
            // between allocates or takes a lock it could hold.
            let eip = unsafe {
                if win::SuspendThread(h) == u32::MAX {
                    None
                } else {
                    let ok = win::GetThreadContext(h, &mut *context);
                    let eip = context.words[win::Context::EIP];
                    // Outside the game: the game's call that led there,
                    // read while the stack holds still.
                    let caller = (ok != 0 && !in_image(eip))
                        .then(|| t.caller(context.words[win::Context::ESP]))
                        .flatten();
                    win::ResumeThread(h);
                    (ok != 0).then_some((eip, caller))
                }
            };
            match eip {
                Some((eip, caller)) => {
                    t.insert(eip);
                    if let Some(c) = caller {
                        count_in(&mut t.callers, c);
                    }
                }
                None => t.failed += 1,
            }
        }
        // SAFETY: a plain Win32 call.
        unsafe { win::Sleep(1) };
    }
    let elapsed = clamp(start.elapsed().as_millis() as u64);
    SAMPLING.store(false, Relaxed);
    Ok(threads.into_iter().map(|t| t.finish(elapsed)).collect())
}

// --- Memory -----------------------------------------------------------------

/// Walks the address space region by region (`VirtualQuery` is slow under
/// Wine: one query per region, at most 2 s).
fn memory() -> MemoryMap {
    let mut m = MemoryMap::default();
    let start = Instant::now();
    let budget = Duration::from_secs(2);
    let mut at = 0x1_0000usize;
    m.finished = true;
    while at < 0xFFFF_0000 {
        if start.elapsed() > budget {
            m.finished = false;
            break;
        }
        let mut info = win::MemoryBasicInformation::default();
        // SAFETY: VirtualQuery only fills `info`.
        let got = unsafe {
            win::VirtualQuery(
                at as *const std::ffi::c_void,
                &mut info,
                std::mem::size_of::<win::MemoryBasicInformation>(),
            )
        };
        if got == 0 || info.region_size == 0 {
            break;
        }
        let base = info.base_address;
        let Some(end) = base.checked_add(info.region_size) else {
            break;
        };
        let size = (end.min(0xFFFF_0000) - base.max(at)) as u64;
        m.regions += 1;
        match info.state {
            win::MEM_COMMIT => {
                m.committed += size;
                if base >= 0x8000_0000 {
                    m.committed_high += size;
                }
            }
            win::MEM_RESERVE => m.reserved += size,
            win::MEM_FREE => {
                m.free += size;
                if size > m.largest_free {
                    m.largest_free = size;
                    m.largest_free_at = base.max(at) as u32;
                }
            }
            _ => {}
        }
        if end <= at {
            break;
        }
        at = end;
    }
    m.walk_ms = clamp(start.elapsed().as_millis() as u64);
    let mut status = win::MemoryStatusEx {
        length: std::mem::size_of::<win::MemoryStatusEx>() as u32,
        ..Default::default()
    };
    // SAFETY: fills `status`, its length set.
    if unsafe { win::GlobalMemoryStatusEx(&mut status) } != 0 {
        m.total_virtual = status.total_virtual;
        m.avail_virtual = status.avail_virtual;
    }
    m
}

// --- The answer ---------------------------------------------------------------

fn clock() -> Clock {
    let mut m = win::Mem::default();
    let mut u16_at = |a: u32| {
        let mut b = [0u8; 2];
        m.bytes(a, &mut b).then(|| u16::from_le_bytes(b))
    };
    let back_buffer = u16_at(BACK_BUFFER_WIDTH).zip(u16_at(BACK_BUFFER_HEIGHT));
    Clock {
        game_frame: m.u32(GAME_FRAME),
        sim_ms: m.u32(SIM_TIME_MS),
        sim_hz: m.f32s::<1>(SIM_HZ).map(|v| v[0]),
        fixed_step: m.u8(FIXED_STEP),
        vsync: m.u8(VSYNC),
        windowed_limiter: m.u8(WINDOWED_LIMITER),
        back_buffer: back_buffer.map(|(w, h)| [w, h]),
    }
}

fn textures() -> Option<TextureUse> {
    let mut table = vec![0u8; TEXTURES * TextureUse::ENTRY];
    win::Mem::default()
        .bytes(TEXTURE_TABLE, &mut table)
        .then(|| TextureUse::from_table(&table))
}

fn reset() {
    PASS_RING.reset();
    INTERVAL_RING.reset();
    PRESENT_RING.reset();
    for io in THREADS.iter().chain(SITE_IO.iter()) {
        io.reset();
    }
    SLOW_N.store(0, Relaxed);
    RESET_AT.store(uptime_us(), Relaxed);
}

/// Answers [`doctor_toolkit_live::Request::Profile`] on the connection's
/// thread.
pub fn answer(do_reset: bool, with_memory: bool, sample_ms: u32) -> Response {
    let samples = if sample_ms > 0 {
        match sample(sample_ms) {
            Ok(s) => s,
            Err(e) => return Response::error(ErrorCode::NotReady, e),
        }
    } else {
        Vec::new()
    };
    let main_thread = MAIN_THREAD.load(Relaxed);
    let render_thread = RENDER_THREAD.load(Relaxed);
    let mut io: Vec<ThreadIo> = THREADS[..IO_THREADS]
        .iter()
        .filter_map(|t| {
            let id = t.thread.load(Relaxed);
            (id != 0).then(|| t.get(id))
        })
        .filter(|t| t.reads > 0)
        .collect();
    let others = THREADS[IO_THREADS].get(0);
    if others.reads > 0 {
        io.push(others);
    }
    let io_sites = SITES
        .iter()
        .zip(SITE_IO.iter())
        .map(|(&site, s)| {
            let t = s.get(0);
            SiteIo {
                site,
                reads: t.reads,
                bytes: t.bytes,
                total_us: t.total_us,
            }
        })
        .filter(|s| s.reads > 0)
        .collect();
    let n = SLOW_N.load(Relaxed);
    let k = n.min(SLOW_RING as u32);
    let slow_reads = (n - k..n)
        .map(|i| {
            let s = &SLOW[i as usize % SLOW_RING];
            SlowRead {
                thread: s.thread.load(Relaxed),
                bytes: s.bytes.load(Relaxed),
                us: s.us.load(Relaxed),
                at_ms: s.at_ms.load(Relaxed),
            }
        })
        .collect();
    let profile = Profile {
        uptime_us: uptime_us(),
        qpc_frequency: FREQUENCY.load(Relaxed),
        reset_at_us: RESET_AT.load(Relaxed),
        main_thread,
        render_thread,
        present_hooked: PRESENT_HOOKED.load(Relaxed),
        reads_hooked: READS_HOOKED.load(Relaxed),
        passes: PASSES.load(Relaxed),
        pass_us: PASS_RING.read(),
        presents: PRESENTS.load(Relaxed),
        present_interval_us: INTERVAL_RING.read(),
        present_us: PRESENT_RING.read(),
        clock: clock(),
        main_cpu: thread_cpu(main_thread),
        render_cpu: thread_cpu(render_thread),
        process_cpu: process_cpu(),
        io,
        io_sites,
        slow_reads,
        textures: textures(),
        memory: with_memory.then(memory),
        samples,
    };
    if do_reset {
        reset();
    }
    Response::Profile(Box::new(profile))
}
