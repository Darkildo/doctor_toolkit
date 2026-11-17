//! Time and frames: the time of day, vsync, the game's limiter and a frame
//! limit of the module (`docs/analysis/performance.md` §7, §10).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use doctor_toolkit_live::render::{Pacer, RenderConfig, addr};

use crate::win;

static START: AtomicU64 = AtomicU64::new(0);
static FREQ: AtomicU64 = AtomicU64::new(1);
static PACER: Mutex<Option<Pacer>> = Mutex::new(None);
/// Presents since the start, and ticks the module waited before them.
static PRESENTS: AtomicU32 = AtomicU32::new(0);
static WAITED: AtomicU64 = AtomicU64::new(0);
/// `force_vsync`: the refreshes the limiter is made to wait, 0 — off.
static FORCE_WAIT: AtomicU32 = AtomicU32::new(0);

pub fn start_clock() {
    FREQ.store(win::frequency(), Ordering::Relaxed);
    START.store(win::now(), Ordering::Relaxed);
    // Sleep(1) sleeps one millisecond, not a tick of 15.6.
    // SAFETY: plain Win32 call.
    unsafe { win::timeBeginPeriod(1) };
}

fn freq() -> u64 {
    FREQ.load(Ordering::Relaxed)
}

/// Seconds since the module loaded.
pub fn seconds() -> f64 {
    let t = win::now().saturating_sub(START.load(Ordering::Relaxed));
    t as f64 / freq() as f64
}

/// The time of day's tick (`0x00826B70`) zeroes its counter of 1/1200 s
/// after giving out the whole seconds: the rest of every frame is lost and
/// a day lasts 20–36 % longer than the level says. Stands in `mov dword
/// [edi], 0` (`call`, then `nop`): keeps `counter mod 1200` instead. EDI
/// points at the counter; EAX, ECX and EDX are free there (the next
/// instructions load them anew).
#[unsafe(naked)]
extern "C" fn keep_rest() {
    core::arch::naked_asm!(
        "mov eax, dword ptr [edi]",
        "xor edx, edx",
        "mov ecx, 1200",
        "div ecx",
        "mov dword ptr [edi], edx",
        "ret",
    )
}

/// Stands in the call of the game's limiter `0x00608CA0(n)` in the
/// present: the same wait — `n` refreshes of 16.667 ms from the last pass,
/// only when the byte `0x0120575A` asks for it — but sleeping instead of
/// spinning `SleepEx(0)`. Full screen at `INTERVAL_ONE` the display paces
/// the frames, and the module does not wait on top of it.
extern "C" fn limiter(n: i32) {
    let r = std::panic::catch_unwind(|| {
        // SAFETY: data of the game proved by the checked places.
        unsafe {
            if win::read::<u8>(addr::LIMITER_ON) == 0 || n < 1 {
                return;
            }
            let windowed = win::read::<u32>(addr::WINDOWED) != 0;
            let interval = win::read::<u32>(addr::INTERVAL);
            if !windowed && interval == 1 {
                win::write::<u64>(addr::LIMITER_STAMP, win::now());
                return;
            }
            let last = win::read::<u64>(addr::LIMITER_STAMP);
            let wait = freq() * n as u64 * 1000 / 60_000;
            let deadline = last.saturating_add(wait);
            let now = win::now();
            if last != 0 && now < deadline && deadline - now <= wait {
                win::wait_until(deadline, freq());
            }
            win::write::<u64>(addr::LIMITER_STAMP, win::now());
        }
    });
    if r.is_err() {
        // SAFETY: the game's own limiter, cdecl, one argument (checked).
        let original: extern "C" fn(i32) = unsafe { std::mem::transmute(addr::LIMITER as usize) };
        original(n);
    }
}

/// Stands in the call of copy-and-`Present` `0x00608E90`: waits for the
/// frame limit first.
extern "C" fn present() {
    let _ = std::panic::catch_unwind(|| {
        if let Ok(mut pacer) = PACER.try_lock()
            && let Some(p) = pacer.as_mut()
        {
            let now = win::now();
            let deadline = p.deadline(now);
            if deadline > now {
                win::wait_until(deadline, freq());
                WAITED.fetch_add(win::now() - now, Ordering::Relaxed);
            }
        }
        PRESENTS.fetch_add(1, Ordering::Relaxed);
        crate::draws::frame();
        let force = FORCE_WAIT.load(Ordering::Relaxed);
        if force != 0 {
            // SAFETY: the u32 read at 0x0060D8DA (checked).
            unsafe { win::write::<u32>(addr::VSYNC_WAIT, force) };
        }
    });
    // SAFETY: the game's copy and Present, cdecl, no arguments (checked).
    let original: extern "C" fn() = unsafe { std::mem::transmute(addr::PRESENT as usize) };
    original();
}

/// Presents so far and the share of time the frame limit waited.
pub fn stats() -> (u32, f64) {
    (
        PRESENTS.load(Ordering::Relaxed),
        WAITED.load(Ordering::Relaxed) as f64 / freq() as f64,
    )
}

pub fn install(config: &RenderConfig, done: &mut Vec<String>) {
    // SAFETY: every place below is checked by `render::check`.
    unsafe {
        if config.time_of_day {
            let rel = (keep_rest as extern "C" fn() as usize as u32)
                .wrapping_sub(addr::TIME_OF_DAY_ZERO + 5)
                .to_le_bytes();
            let bytes = [0xE8, rel[0], rel[1], rel[2], rel[3], 0x90];
            if win::patch(addr::TIME_OF_DAY_ZERO, &bytes) {
                done.push("time of day keeps the rest (0x00826BF5)".into());
            }
        }
        if config.vsync_one {
            let ok = win::patch(addr::INTERVAL_TWO_IMM, &[1])
                && win::patch(addr::VSYNC_WAIT_IMMS[0], &[1])
                && win::patch(addr::VSYNC_WAIT_IMMS[1], &[1]);
            if ok {
                done.push(
                    "vsync waits one refresh (0x006075E2 INTERVAL_ONE, 0x00607B59, 0x006084CE)"
                        .into(),
                );
            }
        }
        if config.limiter_sleep
            && win::redirect_call(addr::LIMITER_CALL, limiter as extern "C" fn(i32) as usize)
        {
            done.push("the limiter sleeps (0x0060D8F2)".into());
        }
        if config.fps > 0
            && let Ok(mut p) = PACER.lock()
        {
            *p = Some(Pacer::new(freq(), config.fps));
        }
        // Without a limit the hook only counts the presents for the log.
        if config.force_vsync {
            FORCE_WAIT.store(if config.vsync_one { 1 } else { 2 }, Ordering::Relaxed);
            done.push("test: the limiter waits as with vsync on (0x00F15A48)".into());
        }
        if (config.fps > 0 || config.memory_interval() > 0 || config.force_vsync || config.draws())
            && win::redirect_call(addr::PRESENT_CALL, present as extern "C" fn() as usize)
            && config.fps > 0
        {
            done.push(format!("frame limit {} a second (0x0060D8FA)", config.fps));
        }
    }
}
