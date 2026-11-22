//! The frame tick and the events of the Direct3D device (stage 115).
//!
//! The tick: the module points the call of the game's tick in the frame of
//! both main loops (`0x0068BC15` → `0x0040EBB0`) to [`tick_hook`], which
//! runs the game's tick and then the listeners of the mods. The frame runs
//! in a level, in its pause menu, in Don's view and in the main menu
//! (`docs/analysis/game-bench.md` §57) — unlike `iMsgRunningTick`, which
//! the game swaps for `iMsgPausedTick` while paused.
//!
//! The device: the game keeps a registry of handlers it calls before and
//! after `Reset` (`0x00607CA0`, 32 entries). The module adds itself there
//! once, when the first mod subscribes — not from `DllMain`: a static
//! initialiser of the game empties the registry after the module loads.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use doctor_sdk::abi::{self, DeviceFn, TickFn};
use doctor_toolkit_live::platform::addr;

use crate::win;

/// Subscriptions of one kind.
struct List<F>(Vec<(i32, F, *mut c_void)>);
// SAFETY: `user` is a pointer of a mod, only handed back to it.
unsafe impl<F> Send for List<F> {}

const MAX: usize = 64;

static NEXT_ID: AtomicI32 = AtomicI32::new(1);
static TICKS: Mutex<List<TickFn>> = Mutex::new(List(Vec::new()));
static DEVICE: Mutex<List<DeviceFn>> = Mutex::new(List(Vec::new()));
static DEVICE_REGISTERED: AtomicBool = AtomicBool::new(false);

/// A new identifier of a subscription, above 0.
pub(crate) fn next_id() -> i32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn add<F>(list: &Mutex<List<F>>, f: F, user: *mut c_void) -> i32 {
    let Ok(mut list) = list.lock() else {
        return abi::ERR_ENGINE;
    };
    if list.0.len() >= MAX {
        return abi::ERR_FULL;
    }
    let id = next_id();
    list.0.push((id, f, user));
    id
}

fn remove<F>(list: &Mutex<List<F>>, id: i32) -> i32 {
    let Ok(mut list) = list.lock() else {
        return abi::ERR_ENGINE;
    };
    let before = list.0.len();
    list.0.retain(|e| e.0 != id);
    if list.0.len() == before {
        abi::ERR_ARGUMENT
    } else {
        abi::OK
    }
}

/// The listeners, copied: a listener may subscribe or unsubscribe.
fn copy<F: Copy>(list: &Mutex<List<F>>) -> Vec<(F, *mut c_void)> {
    match list.lock() {
        Ok(l) => l.0.iter().map(|e| (e.1, e.2)).collect(),
        Err(_) => Vec::new(),
    }
}

// --- The tick ---------------------------------------------------------------------

/// What stands in for the call of the game's tick at `0x0068BC15`.
pub extern "C" fn tick_hook() {
    // SAFETY: the function the call called before (checked bytes), cdecl
    // without arguments.
    unsafe {
        let tick: unsafe extern "C" fn() = std::mem::transmute(addr::TICK_CALLEE as usize);
        tick();
    }
    let _ = std::panic::catch_unwind(|| {
        let listeners = copy(&TICKS);
        if listeners.is_empty() {
            return;
        }
        // SAFETY: a word in the data of the game.
        let mode = unsafe { win::read::<u32>(addr::TICK_MODE) };
        for (f, user) in listeners {
            // SAFETY: a function of a mod, on the main thread.
            unsafe { f(user, mode) };
        }
    });
}

pub fn tick_listen(f: TickFn, user: *mut c_void) -> i32 {
    add(&TICKS, f, user)
}

pub fn tick_unlisten(id: i32) -> i32 {
    remove(&TICKS, id)
}

// --- The device -------------------------------------------------------------------

fn device_event(event: u32) {
    let _ = std::panic::catch_unwind(|| {
        for (f, user) in copy(&DEVICE) {
            // SAFETY: a function of a mod, on the thread that resets the
            // device.
            unsafe { f(user, event) };
        }
    });
}

/// The game calls it before `Reset` (cdecl, one argument of its own).
extern "C" fn on_lost(_: u32) {
    device_event(abi::DEVICE_LOST);
    let _ = std::panic::catch_unwind(crate::overlay::device_lost);
}

/// The game calls it after `Reset`.
extern "C" fn on_reset(_: u32) {
    device_event(abi::DEVICE_RESET);
}

/// Adds the module to the game's registry once.
pub(crate) fn register_device() -> Result<(), i32> {
    if DEVICE_REGISTERED.load(Ordering::Relaxed) {
        return Ok(());
    }
    // SAFETY: the registration of the game (checked bytes), cdecl; its
    // count is a word in the data of the game.
    let (before, after) = unsafe {
        let before = win::read::<u32>(addr::DEVICE_COUNT);
        let register: unsafe extern "C" fn(extern "C" fn(u32), extern "C" fn(u32), u32) =
            std::mem::transmute(addr::DEVICE_REGISTER as usize);
        register(on_lost, on_reset, u32::from(addr::DEVICE_APPEND));
        (before, win::read::<u32>(addr::DEVICE_COUNT))
    };
    if after != before + 1 {
        crate::log(&format!(
            "device registry: not added ({before} of {} entries)",
            addr::DEVICE_CAPACITY
        ));
        return Err(abi::ERR_FULL);
    }
    DEVICE_REGISTERED.store(true, Ordering::Relaxed);
    crate::log(&format!(
        "device registry: added as entry {after} of {}",
        addr::DEVICE_CAPACITY
    ));
    Ok(())
}

pub fn device_listen(f: DeviceFn, user: *mut c_void) -> i32 {
    if let Err(code) = register_device() {
        return code;
    }
    add(&DEVICE, f, user)
}

pub fn device_unlisten(id: i32) -> i32 {
    remove(&DEVICE, id)
}
