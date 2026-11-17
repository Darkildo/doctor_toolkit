//! Messages: sending, listeners that are not objects, and the filter of
//! sends to all listeners.
//!
//! Everything goes through the functions of the engine's bus
//! (`docs/analysis/message-bus.md` §2); only the filter needs a patch, and
//! it is installed when the first mod asks for one.

use std::arch::global_asm;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use doctor_sdk::abi::{self, FilterFn, Message, MessageFn};
use doctor_toolkit_live::platform::addr;

use crate::win::{self, read};

/// Whether the table of messages exists yet.
pub fn ready() -> bool {
    // SAFETY: a pointer in the data of the game.
    unsafe { read::<u32>(addr::MESSAGE_TABLE) != 0 }
}

/// Names a message variable `{hash, u16, u16}`.
///
/// # Safety
/// The bus must be [`ready`]; `variable` must stay where it is while it is
/// named.
pub unsafe fn name(variable: *mut [u32; 2], msg: u32) {
    // SAFETY: `0x00408260(variable, hash)`, cdecl.
    unsafe {
        let f: unsafe extern "C" fn(*mut [u32; 2], u32) =
            std::mem::transmute(addr::MESSAGE_VARIABLE as usize);
        f(variable, msg);
    }
}

/// Releases a named variable.
///
/// # Safety
/// As [`name`].
pub unsafe fn release(variable: *mut [u32; 2]) {
    // SAFETY: `0x00408310(variable)`, cdecl.
    unsafe {
        let f: unsafe extern "C" fn(*mut [u32; 2]) =
            std::mem::transmute(addr::MESSAGE_RELEASE as usize);
        f(variable);
    }
}

/// Subscribes a listener (`{method table, flags, subscriptions}`).
///
/// # Safety
/// `listener` must be a listener of the engine's layout that outlives the
/// subscription; `variable` a named variable.
pub unsafe fn subscribe(listener: u32, variable: *mut [u32; 2], priority: u32) {
    // SAFETY: `0x00408900(listener, variable, priority)`, cdecl.
    unsafe {
        let f: unsafe extern "C" fn(u32, *mut [u32; 2], u32) =
            std::mem::transmute(addr::MESSAGE_SUBSCRIBE as usize);
        f(listener, variable, priority);
    }
}

/// Ends a subscription.
///
/// # Safety
/// As [`subscribe`].
pub unsafe fn unsubscribe(listener: u32, variable: *mut [u32; 2]) {
    // SAFETY: `0x004086D0`, thiscall of the listener.
    unsafe {
        let f: unsafe extern "thiscall" fn(u32, *mut [u32; 2]) =
            std::mem::transmute(addr::MESSAGE_UNSUBSCRIBE as usize);
        f(listener, variable);
    }
}

/// Sends to every listener; whether somebody listened.
pub fn send(msg: u32, word: u32, force: u32) -> i32 {
    if msg == 0 {
        return abi::ERR_ARGUMENT;
    }
    if !ready() {
        return abi::ERR_STATE;
    }
    let mut message = Message {
        hash: msg,
        data: word,
        delivering: 0,
    };
    // SAFETY: `bool Send(Message*, bool force)`, cdecl.
    unsafe {
        let f: unsafe extern "C" fn(*mut Message, u32) -> u8 =
            std::mem::transmute(addr::MESSAGE_SEND as usize);
        i32::from(f(&mut message, u32::from(force != 0)) != 0)
    }
}

/// Sends with data that points to an object, built on the stack as the
/// objects of the game build it.
pub fn send_object(msg: u32, about: u32, force: u32) -> i32 {
    if about == 0 {
        return send(msg, 0, force);
    }
    let mut data = [0u32; 3];
    // SAFETY: the constructor and the destructor of the engine's pointer
    // to an object (`0x0045CA00`, `0x0045CAC0`), thiscall.
    unsafe {
        let ctor: unsafe extern "thiscall" fn(*mut [u32; 3], u32) -> u32 =
            std::mem::transmute(addr::DATA_OBJECT_CTOR as usize);
        let dtor: unsafe extern "thiscall" fn(*mut [u32; 3]) =
            std::mem::transmute(addr::DATA_OBJECT_DTOR as usize);
        ctor(&mut data, about);
        let sent = send(msg, (&raw mut data) as u32, force);
        dtor(&mut data);
        sent
    }
}

/// Delivers to the listener of one object.
pub fn send_to(msg: u32, word: u32, to: u32, force: u32) -> i32 {
    if msg == 0 || to == 0 {
        return abi::ERR_ARGUMENT;
    }
    if !ready() {
        return abi::ERR_STATE;
    }
    let mut message = Message {
        hash: msg,
        data: word,
        delivering: 0,
    };
    // SAFETY: `SendTo(Message*, Listener*, bool force)`, cdecl; the
    // listener of an object lies at `+0x3C`.
    unsafe {
        let f: unsafe extern "C" fn(*mut Message, u32, u32) =
            std::mem::transmute(addr::MESSAGE_SEND_TO as usize);
        f(
            &mut message,
            to + addr::OBJECT_LISTENER,
            u32::from(force != 0),
        );
    }
    abi::OK
}

/// Sends to every listener after `seconds` of game time.
pub fn send_later(msg: u32, seconds: f32, about: u32) -> i32 {
    if msg == 0 || !seconds.is_finite() {
        return abi::ERR_ARGUMENT;
    }
    if seconds <= 0.0 {
        let sent = send_object(msg, about, 0);
        return if sent < 0 { sent } else { abi::OK };
    }
    // SAFETY: the queue of delayed messages `*0x01223420` and its send
    // (`0x00440590`, thiscall); the data is made as `TriggerAuto` makes it
    // (`0x00477F7A`): 12 bytes of the game's `new`, deleted by the queue.
    unsafe {
        let queue: u32 = read(addr::MESSAGE_QUEUE);
        if queue == 0 || !ready() {
            return abi::ERR_STATE;
        }
        let mut data = 0u32;
        if about != 0 {
            let new: unsafe extern "C" fn(u32) -> u32 = std::mem::transmute(addr::NEW as usize);
            data = new(addr::DATA_OBJECT_SIZE);
            if data == 0 {
                return abi::ERR_ENGINE;
            }
            let ctor: unsafe extern "thiscall" fn(u32, u32) -> u32 =
                std::mem::transmute(addr::DATA_OBJECT_CTOR as usize);
            ctor(data, about);
        }
        let mut variable = [0u32; 2];
        name(&mut variable, msg);
        let later: unsafe extern "thiscall" fn(u32, *mut [u32; 2], u32, f32, u32, u32) =
            std::mem::transmute(addr::MESSAGE_SEND_LATER as usize);
        later(queue, &mut variable, 0, seconds, data, 0);
        release(&mut variable);
    }
    abi::OK
}

/// The object a data word points to, when it is the engine's pointer to an
/// object: `{0x00E31E2C, anchor}`, the anchor being the object `+ 0x48`.
pub fn data_object(word: u32) -> u32 {
    if !win::readable(word, 8) {
        return 0;
    }
    // SAFETY: readable, checked above.
    unsafe {
        if read::<u32>(word) != addr::DATA_OBJECT_VTABLE {
            return 0;
        }
        match read::<u32>(word + 4) {
            0 => 0,
            anchor => anchor - addr::OBJECT_THIRD,
        }
    }
}

// --- Listeners that are not objects ------------------------------------------------

/// A listener of a mod: the three words of the engine's listener, then
/// ours.
#[repr(C)]
struct Listener {
    vtable: *const [u32; 4],
    flags: u32,
    subscriptions: u32,
    variable: [u32; 2],
    function: MessageFn,
    user: *mut c_void,
    live: bool,
}

unsafe extern "thiscall" fn listener_dtor(this: u32, _flags: u32) -> u32 {
    this
}

unsafe extern "thiscall" fn listener_handler(this: *mut Listener, message: u32) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: a listener made by `subscribe_function`, never freed; the
        // message the bus is delivering.
        unsafe {
            if (*this).live {
                ((*this).function)((*this).user, read(message), read(message + 4));
            }
        }
    });
}

unsafe extern "thiscall" fn listener_off(this: *mut Listener) {
    // SAFETY: as above.
    unsafe { (*this).flags &= !1 }
}

unsafe extern "thiscall" fn listener_on(this: *mut Listener) {
    // SAFETY: as above.
    unsafe { (*this).flags |= 1 }
}

static mut LISTENER_VTABLE: [u32; 4] = [0; 4];

struct Listeners(Vec<*mut Listener>);
// SAFETY: used on the main thread of the game; the lock only guards the list.
unsafe impl Send for Listeners {}
static LISTENERS: Mutex<Listeners> = Mutex::new(Listeners(Vec::new()));

/// Fills the method table of the listeners. Once.
pub fn prepare() {
    // SAFETY: written once before any listener exists.
    unsafe {
        let table = &raw mut LISTENER_VTABLE;
        *table = [
            listener_dtor as unsafe extern "thiscall" fn(u32, u32) -> u32 as usize as u32,
            listener_handler as unsafe extern "thiscall" fn(*mut Listener, u32) as usize as u32,
            listener_off as unsafe extern "thiscall" fn(*mut Listener) as usize as u32,
            listener_on as unsafe extern "thiscall" fn(*mut Listener) as usize as u32,
        ];
    }
}

/// Subscribes a function of a mod; its identifier (from 1).
pub fn subscribe_function(msg: u32, priority: u32, function: MessageFn, user: *mut c_void) -> i32 {
    if msg == 0 || priority > 0xFFFF {
        return abi::ERR_ARGUMENT;
    }
    if !ready() {
        return abi::ERR_STATE;
    }
    let Ok(mut list) = LISTENERS.lock() else {
        return abi::ERR_ENGINE;
    };
    if list.0.len() >= 4096 {
        return abi::ERR_FULL;
    }
    let listener = Box::into_raw(Box::new(Listener {
        vtable: &raw const LISTENER_VTABLE,
        flags: 1,
        subscriptions: 0,
        variable: [0; 2],
        function,
        user,
        live: true,
    }));
    list.0.push(listener);
    let id = list.0.len() as i32;
    drop(list);
    // SAFETY: a listener of the engine's layout that is never freed.
    unsafe {
        name(&raw mut (*listener).variable, msg);
        subscribe(listener as u32, &raw mut (*listener).variable, priority);
    }
    id
}

/// Ends a subscription made by [`subscribe_function`].
pub fn unsubscribe_function(id: i32) -> i32 {
    let listener = match LISTENERS.lock() {
        Ok(list) if id >= 1 => list.0.get(id as usize - 1).copied(),
        _ => None,
    };
    let Some(listener) = listener else {
        return abi::ERR_ARGUMENT;
    };
    // SAFETY: made by `subscribe_function`.
    unsafe {
        if !(*listener).live {
            return abi::ERR_ARGUMENT;
        }
        (*listener).live = false;
        unsubscribe(listener as u32, &raw mut (*listener).variable);
        release(&raw mut (*listener).variable);
    }
    abi::OK
}

// --- The filter --------------------------------------------------------------------

struct Filters(Vec<(FilterFn, *mut c_void)>);
// SAFETY: as `Listeners`.
unsafe impl Send for Filters {}
static FILTERS: Mutex<Filters> = Mutex::new(Filters(Vec::new()));
static FILTER_HOOKED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_send(message: *mut Message, force: u32) -> u32 {
    std::panic::catch_unwind(|| {
        // The list is copied: a filter may send and come back here.
        let filters = match FILTERS.lock() {
            Ok(f) => f.0.clone(),
            Err(_) => return 0,
        };
        for (filter, user) in filters {
            // SAFETY: a function of a mod; the message the bus is sending.
            if unsafe { filter(user, message, force & 0xFF) } != 0 {
                return 1;
            }
        }
        0
    })
    .unwrap_or(0)
}

// The thunk `Send` jumps to from 0x00408A1A. Stack there: saved EDI, ESI,
// ECX, the return address of Send, the message, `force`; the message is in
// EDX too. It saves every register and calls `on_send(message, force)`;
// then runs the instructions the jump took — `xor al, al; test esi, esi;
// je 0x00408AE4` — and goes on at 0x00408A24, or, when a filter swallowed
// the message, returns `false` through the end of Send at 0x00408AE4.
global_asm!(
    ".globl _doctor_platform_filter_thunk",
    "_doctor_platform_filter_thunk:",
    "pushad",
    "pushfd",
    // Flags and 8 registers (36 bytes), EDI, ESI, ECX, the return, the
    // message, then `force` at +56.
    "push dword ptr [esp + 56]",
    "push edx",
    "call {on_send}",
    "add esp, 8",
    "test eax, eax",
    "jnz 2f",
    "popfd",
    "popad",
    "xor al, al",
    "test esi, esi",
    "jz 3f",
    "push {resume}",
    "ret",
    "2:",
    "popfd",
    "popad",
    "xor al, al",
    "3:",
    "push {miss}",
    "ret",
    on_send = sym on_send,
    resume = const addr::FILTER_RESUME,
    miss = const addr::FILTER_MISS,
);

unsafe extern "C" {
    fn doctor_platform_filter_thunk();
}

/// Adds a filter; the first one installs the hook.
pub fn add_filter(filter: FilterFn, user: *mut c_void) -> i32 {
    let Ok(mut filters) = FILTERS.lock() else {
        return abi::ERR_ENGINE;
    };
    if filters.0.len() >= 64 {
        return abi::ERR_FULL;
    }
    if !FILTER_HOOKED.load(Ordering::Relaxed) {
        let thunk = doctor_platform_filter_thunk as unsafe extern "C" fn() as usize as u32;
        let mut code = [0x90u8; addr::FILTER_HOOK_LEN as usize];
        code[0] = 0xE9;
        code[1..5].copy_from_slice(&thunk.wrapping_sub(addr::FILTER_HOOK + 5).to_le_bytes());
        // SAFETY: ten bytes of three whole instructions, checked by the
        // edition check, that the thunk runs instead; patched on the main
        // thread, the only one that sends.
        if !unsafe { win::patch(addr::FILTER_HOOK, &code) } {
            return abi::ERR_ENGINE;
        }
        FILTER_HOOKED.store(true, Ordering::Relaxed);
        crate::log(&format!(
            "message filter: hook at {:#010x}",
            addr::FILTER_HOOK
        ));
    }
    filters.0.push((filter, user));
    abi::OK
}
