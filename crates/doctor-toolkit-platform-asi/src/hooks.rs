//! Hooks of native mods in the code of the game.
//!
//! One list answers "may I patch here": the places the modules of the
//! toolkit patch and the places they check
//! ([`doctor_toolkit_live::platform::mod_hook_allowed`] — the same list the
//! tests of the toolkit hold its own modules to), plus the places mods have
//! taken while the game runs. A place is given once.

use std::ffi::c_void;
use std::sync::Mutex;

use doctor_sdk::abi;
use doctor_toolkit_live::platform::{self, HookRefusal};

use crate::win::{self, read};

/// Places taken by mods: address and length.
static TAKEN: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// Room for trampolines: 32 bytes each.
const TRAMPOLINE: usize = 32;
const TRAMPOLINES: usize = 128;
struct Pool {
    base: u32,
    used: usize,
}
static POOL: Mutex<Pool> = Mutex::new(Pool { base: 0, used: 0 });

fn refusal(address: u32, len: u32) -> Option<(i32, String)> {
    match platform::mod_hook_allowed(address, len) {
        Err(HookRefusal::NotCode) => {
            return Some((abi::ERR_ARGUMENT, HookRefusal::NotCode.to_string()));
        }
        Err(why) => return Some((abi::ERR_TAKEN, why.to_string())),
        Ok(()) => {}
    }
    let taken = TAKEN.lock().ok()?;
    taken
        .iter()
        .find(|&&(a, l)| platform::overlap(address, len, a, l))
        .map(|&(a, _)| {
            (
                abi::ERR_TAKEN,
                format!("another mod has a hook at {a:#010x}"),
            )
        })
}

/// Whether a mod may patch the place.
pub fn check(address: u32, len: u32) -> i32 {
    refusal(address, len).map_or(abi::OK, |(code, _)| code)
}

fn take(address: u32, len: u32) {
    if let Ok(mut taken) = TAKEN.lock() {
        taken.push((address, len));
    }
}

fn trampoline() -> Option<u32> {
    let mut pool = POOL.lock().ok()?;
    if pool.base == 0 {
        // SAFETY: a fresh executable page of the module's own.
        pool.base = unsafe {
            win::VirtualAlloc(
                std::ptr::null_mut(),
                TRAMPOLINE * TRAMPOLINES,
                win::MEM_COMMIT | win::MEM_RESERVE,
                win::PAGE_EXECUTE_READWRITE,
            )
        } as u32;
    }
    if pool.base == 0 || pool.used >= TRAMPOLINES {
        return None;
    }
    pool.used += 1;
    Some(pool.base + ((pool.used - 1) * TRAMPOLINE) as u32)
}

/// Replaces `len` bytes at `address` with a jump to `detour`; the
/// trampoline runs the replaced instructions and goes on after them.
pub fn jump(address: u32, len: u32, detour: *const c_void, out: *mut *const c_void) -> i32 {
    if detour.is_null() || out.is_null() || !(5..=16).contains(&len) {
        return abi::ERR_ARGUMENT;
    }
    if let Some((code, why)) = refusal(address, len) {
        crate::log(&format!("hook at {address:#010x} (+{len}) refused: {why}"));
        return code;
    }
    let Some(tramp) = trampoline() else {
        return abi::ERR_FULL;
    };
    // SAFETY: code of the game, readable; the trampoline is ours.
    unsafe {
        let mut bytes = [0u8; 16 + 5];
        let n = len as usize;
        std::ptr::copy_nonoverlapping(address as usize as *const u8, bytes.as_mut_ptr(), n);
        // A first `call`/`jmp rel32` keeps its target from the new place.
        if bytes[0] == 0xE8 || bytes[0] == 0xE9 {
            let target = (address + 5).wrapping_add(read::<u32>(address + 1));
            bytes[1..5].copy_from_slice(&target.wrapping_sub(tramp + 5).to_le_bytes());
        }
        bytes[n] = 0xE9;
        let back = (address + len).wrapping_sub(tramp + len + 5);
        bytes[n + 1..n + 5].copy_from_slice(&back.to_le_bytes());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), tramp as usize as *mut u8, n + 5);
        win::FlushInstructionCache(
            win::GetCurrentProcess(),
            tramp as usize as *const c_void,
            n + 5,
        );
        *out = tramp as usize as *const c_void;
        let mut code = [0x90u8; 16];
        code[0] = 0xE9;
        code[1..5].copy_from_slice(&(detour as u32).wrapping_sub(address + 5).to_le_bytes());
        if !win::patch(address, &code[..n]) {
            return abi::ERR_ENGINE;
        }
    }
    take(address, len);
    crate::log(&format!(
        "hook: jump at {address:#010x} (+{len}) to {:#010x}",
        detour as u32
    ));
    abi::OK
}

/// Points the `call rel32` at `address` to `target`.
pub fn call(address: u32, target: *const c_void, out: *mut *const c_void) -> i32 {
    if target.is_null() || out.is_null() {
        return abi::ERR_ARGUMENT;
    }
    if let Some((code, why)) = refusal(address, 5) {
        crate::log(&format!(
            "hook of the call at {address:#010x} refused: {why}"
        ));
        return code;
    }
    // SAFETY: code of the game, readable.
    unsafe {
        if read::<u8>(address) != 0xE8 {
            return abi::ERR_ARGUMENT;
        }
        let original = (address + 5).wrapping_add(read::<u32>(address + 1));
        *out = original as usize as *const c_void;
        let rel = (target as u32).wrapping_sub(address + 5);
        if !win::patch(address + 1, &rel.to_le_bytes()) {
            return abi::ERR_ENGINE;
        }
    }
    take(address, 5);
    crate::log(&format!(
        "hook: call at {address:#010x} to {:#010x}",
        target as u32
    ));
    abi::OK
}

/// Number of hooks mods have.
pub fn count() -> usize {
    TAKEN.lock().map_or(0, |t| t.len())
}
