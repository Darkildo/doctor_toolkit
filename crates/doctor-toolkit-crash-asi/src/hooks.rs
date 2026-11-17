//! What the module remembers for a report while the game runs: the last
//! messages of the bus and the last errors of Lua.
//!
//! Two hooks of the module's own, at places the live module and the
//! graphics module neither patch nor check
//! ([`doctor_toolkit_live::crash`]):
//!
//! - the message bus (`Send`, `0x00408A00`) at its instruction `and esi,
//!   0xFFF` (`0x00408A11`): the message is in `EDX`, the address `Send`
//!   returns to — the sender — on the stack;
//! - `luaD_throw` (`0x00635A80`): `L` in `ECX`, the error code in `EDX`,
//!   the message on top of the Lua stack.
//!
//! Each hook writes into a fixed table under a lock it never waits for,
//! allocates nothing and runs the instructions its jump replaced.

use std::arch::global_asm;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use doctor_toolkit_live::crash::{
    BUS_HOOK, BUS_HOOK_BYTES, BUS_HOOK_RETURN, LUA_THROW, LUA_THROW_BYTES, LUA_THROW_RETURN,
};

use crate::win::{self, Mem, Spin};

/// The last sends in order.
pub const RECENT: usize = 48;
/// Distinct messages remembered with their counts.
pub const DISTINCT: usize = 160;
/// Errors of Lua remembered.
pub const ERRORS: usize = 8;
/// Bytes of an error's text kept.
pub const ERROR_TEXT: usize = 400;

/// One send of the bus.
#[derive(Clone, Copy, Default)]
pub struct Sent {
    pub hash: u32,
    pub data: u32,
    /// Where `Send` returns to.
    pub caller: u32,
    pub tick: u32,
    pub thread: u32,
    /// Number of the send, from 1.
    pub seq: u32,
}

/// A message with how many times it was sent.
#[derive(Clone, Copy, Default)]
pub struct Counted {
    pub last: Sent,
    pub count: u32,
}

pub struct Bus {
    pub seq: u32,
    pub recent: [Sent; RECENT],
    pub distinct: [Counted; DISTINCT],
}

/// One error of Lua.
#[derive(Clone, Copy)]
pub struct LuaError {
    pub tick: u32,
    pub thread: u32,
    pub code: u32,
    /// The same text in a row.
    pub count: u32,
    pub len: usize,
    pub text: [u8; ERROR_TEXT],
}

pub struct Errors {
    pub total: u32,
    pub next: usize,
    pub list: [Option<LuaError>; ERRORS],
}

const NO_SEND: Sent = Sent {
    hash: 0,
    data: 0,
    caller: 0,
    tick: 0,
    thread: 0,
    seq: 0,
};

pub static BUS: Spin<Bus> = Spin::new(Bus {
    seq: 0,
    recent: [NO_SEND; RECENT],
    distinct: [Counted {
        last: NO_SEND,
        count: 0,
    }; DISTINCT],
});

pub static LUA: Spin<Errors> = Spin::new(Errors {
    total: 0,
    next: 0,
    list: [None; ERRORS],
});

/// A crash asked for by `crash-now.txt`: 1 — a read of address 0x14, 2 — a
/// call of `0x6E756F7B`; done at the next message of the bus.
pub static TEST: AtomicU8 = AtomicU8::new(0);
/// Which of the hooks are in place: bit 0 the bus, bit 1 Lua.
pub static INSTALLED: AtomicU32 = AtomicU32::new(0);

/// How long a hook tries for a table before it drops its record.
const HOOK_SPINS: u32 = 2_000;

extern "C" fn on_bus(message: u32, caller: u32) {
    match TEST.swap(0, Ordering::Relaxed) {
        // SAFETY: none — a crash on purpose, on a thread of the game.
        1 => unsafe {
            std::ptr::read_volatile(0x14 as *const u32);
        },
        2 => {
            // SAFETY: none — the known crash address, outside every module.
            let nowhere: extern "C" fn() = unsafe { std::mem::transmute(0x6E75_6F7Busize) };
            nowhere();
        }
        _ => {}
    }
    // SAFETY: the bus itself read `[edx]` two instructions earlier.
    let (hash, data) = unsafe {
        let m = message as usize as *const u32;
        (m.read_unaligned(), m.add(1).read_unaligned())
    };
    // SAFETY: plain Win32 calls.
    let (tick, thread) = unsafe { (win::GetTickCount(), win::GetCurrentThreadId()) };
    BUS.with(HOOK_SPINS, |bus| {
        bus.seq = bus.seq.wrapping_add(1);
        let sent = Sent {
            hash,
            data,
            caller,
            tick,
            thread,
            seq: bus.seq,
        };
        bus.recent[bus.seq as usize % RECENT] = sent;
        // The message's own line, else the line unused for the longest.
        let mut slot = 0;
        let mut oldest = u32::MAX;
        for (i, c) in bus.distinct.iter().enumerate() {
            if c.count != 0 && c.last.hash == hash {
                slot = i;
                break;
            }
            let age = if c.count == 0 { 0 } else { c.last.seq };
            if age < oldest {
                oldest = age;
                slot = i;
            }
        }
        let line = &mut bus.distinct[slot];
        if line.count == 0 || line.last.hash != hash {
            line.count = 0;
        }
        line.count = line.count.saturating_add(1);
        line.last = sent;
    });
}

/// The text of a Lua error: the value on top of the stack of `l`
/// (`L->top − 8`: type 4 — a string; `TString`: length `+0xC`, text
/// `+0x10`).
fn error_text(l: u32, code: u32, out: &mut [u8; ERROR_TEXT]) -> usize {
    fn put(out: &mut [u8; ERROR_TEXT], text: &[u8]) -> usize {
        let n = text.len().min(ERROR_TEXT);
        out[..n].copy_from_slice(&text[..n]);
        n
    }
    if code == 4 {
        return put(out, b"(not enough memory)");
    }
    let mut m = Mem::default();
    let top = m.ptr(l + 8);
    let value = top.and_then(|top| Some((m.u32(top - 8)?, m.u32(top - 4)?)));
    let Some((kind, value)) = value else {
        return put(out, b"(the Lua stack cannot be read)");
    };
    if kind != 4 {
        return put(out, b"(the error value is not a string)");
    }
    let n = (m.u32(value + 0xC).unwrap_or(0) as usize).min(ERROR_TEXT);
    if n == 0 || !m.bytes(value + 0x10, &mut out[..n]) {
        return put(out, b"(the error string cannot be read)");
    }
    n
}

extern "C" fn on_throw(l: u32, code: u32) {
    let mut text = [0u8; ERROR_TEXT];
    let len = error_text(l, code, &mut text);
    // SAFETY: plain Win32 calls.
    let (tick, thread) = unsafe { (win::GetTickCount(), win::GetCurrentThreadId()) };
    LUA.with(HOOK_SPINS, |errors| {
        errors.total = errors.total.wrapping_add(1);
        let last = (errors.next + ERRORS - 1) % ERRORS;
        if let Some(e) = &mut errors.list[last]
            && e.code == code
            && e.len == len
            && e.text[..len] == text[..len]
        {
            e.count = e.count.saturating_add(1);
            e.tick = tick;
            return;
        }
        errors.list[errors.next] = Some(LuaError {
            tick,
            thread,
            code,
            count: 1,
            len,
            text,
        });
        errors.next = (errors.next + 1) % ERRORS;
    });
}

// The thunk the bus jumps to from 0x00408A11. Stack there: saved EDI, ESI,
// ECX, the return address of Send, the message, `force`. It saves every
// register, calls `on_bus(message, return address)`, restores them, runs
// the instruction the jump took (`and esi, 0xFFF`; its flags are not used)
// and goes on at 0x00408A17.
global_asm!(
    ".globl _doctor_crash_bus_thunk",
    "_doctor_crash_bus_thunk:",
    "pushad",
    "pushfd",
    // Flags and 8 registers (36 bytes), then EDI, ESI, ECX, the return.
    "push dword ptr [esp + 48]",
    "push edx",
    "call {on_bus}",
    "add esp, 8",
    "popfd",
    "popad",
    "and esi, 0xFFF",
    "push {resume}",
    "ret",
    on_bus = sym on_bus,
    resume = const BUS_HOOK_RETURN,
);

// The thunk at the start of luaD_throw (L in ECX, the code in EDX): saves
// every register, calls `on_throw(L, code)`, restores them, runs the five
// bytes the jump took (`mov eax, [ecx+0x50]; test eax, eax` — the next
// instruction tests those flags) and goes on at 0x00635A85.
global_asm!(
    ".globl _doctor_crash_throw_thunk",
    "_doctor_crash_throw_thunk:",
    "pushad",
    "pushfd",
    "push edx",
    "push ecx",
    "call {on_throw}",
    "add esp, 8",
    "popfd",
    "popad",
    "mov eax, dword ptr [ecx + 0x50]",
    "test eax, eax",
    "push {resume}",
    "ret",
    on_throw = sym on_throw,
    resume = const LUA_THROW_RETURN,
);

unsafe extern "C" {
    fn doctor_crash_bus_thunk();
    fn doctor_crash_throw_thunk();
}

/// Writes `jmp target` and `nop`s over `len` bytes of code at `at`.
///
/// # Safety
/// `at` must start `len` (5 or more) bytes of whole instructions without
/// relative operands, checked by the edition check, that the target runs
/// instead.
unsafe fn jump(at: u32, len: usize, target: u32) -> bool {
    let mut code = [0x90u8; 8];
    code[0] = 0xE9;
    code[1..5].copy_from_slice(&target.wrapping_sub(at + 5).to_le_bytes());
    // SAFETY: the caller's contract.
    unsafe { win::patch(at, &code[..len]) }
}

/// Installs both hooks (from `DllMain`, before the game's threads run the
/// code). Returns what could not be installed.
pub fn install() -> Vec<&'static str> {
    let mut failed = Vec::new();
    let bus = doctor_crash_bus_thunk as unsafe extern "C" fn() as usize as u32;
    // SAFETY: six bytes of one instruction, checked by the edition check.
    if unsafe { jump(BUS_HOOK, BUS_HOOK_BYTES.len(), bus) } {
        INSTALLED.fetch_or(1, Ordering::Relaxed);
    } else {
        failed.push("the message bus");
    }
    let throw = doctor_crash_throw_thunk as unsafe extern "C" fn() as usize as u32;
    // SAFETY: five bytes of two instructions, checked by the edition check.
    if unsafe { jump(LUA_THROW, LUA_THROW_BYTES.len(), throw) } {
        INSTALLED.fetch_or(2, Ordering::Relaxed);
    } else {
        failed.push("luaD_throw");
    }
    failed
}
