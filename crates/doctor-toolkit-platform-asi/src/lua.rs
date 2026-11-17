//! New global functions of Lua.
//!
//! The engine registers its functions through "binders": small objects kept
//! by the LuaManager (`*0x012233F4`, list `+0x38`), whose method `+4` is
//! called with every Lua state — when the binder is added (`0x0043E2E0`)
//! and when a state is made later (`0x0043E790`, `0x0043E5D0`). The module
//! adds one binder of its own each time a LuaManager is constructed (the
//! hook of the call at `0x00681860`); its method registers the functions of
//! all mods the way the engine's code does: `lua_pushstring`,
//! `lua_pushcclosure`, `lua_settable(L, LUA_GLOBALSINDEX)`.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use doctor_sdk::abi::{self, LuaFn, LuaState};
use doctor_toolkit_live::platform::addr;

use crate::win::read;

struct Functions(Vec<(*const c_char, LuaFn)>);
// SAFETY: names are static strings of mods; used on the main thread.
unsafe impl Send for Functions {}
static FUNCTIONS: Mutex<Functions> = Mutex::new(Functions(Vec::new()));

/// States the binder has registered into, counted for the log.
pub static STATES: AtomicU32 = AtomicU32::new(0);
/// The LuaManager the binder was last added to.
static ADDED_TO: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
struct Binder {
    vtable: *const BinderVtable,
}

#[repr(C)]
struct BinderVtable {
    destroy: unsafe extern "thiscall" fn(*mut Binder, u32) -> *mut Binder,
    register: unsafe extern "thiscall" fn(*mut Binder, *mut LuaState),
    unregister: unsafe extern "thiscall" fn(*mut Binder, *mut LuaState),
}

unsafe extern "thiscall" fn binder_destroy(this: *mut Binder, _flags: u32) -> *mut Binder {
    this
}

unsafe extern "thiscall" fn binder_register(_: *mut Binder, state: *mut LuaState) {
    let _ = std::panic::catch_unwind(|| {
        let functions = match FUNCTIONS.lock() {
            Ok(f) => f.0.clone(),
            Err(_) => return,
        };
        for (name, function) in functions {
            // SAFETY: a state the LuaManager gives to its binders.
            unsafe { set_global(state, name, function) };
        }
        STATES.fetch_add(1, Ordering::Relaxed);
    });
}

unsafe extern "thiscall" fn binder_unregister(_: *mut Binder, _: *mut LuaState) {}

static BINDER_VTABLE: BinderVtable = BinderVtable {
    destroy: binder_destroy,
    register: binder_register,
    unregister: binder_unregister,
};

struct TheBinder(Binder);
// SAFETY: a table pointer that never changes.
unsafe impl Sync for TheBinder {}
static BINDER: TheBinder = TheBinder(Binder {
    vtable: &BINDER_VTABLE,
});

/// `name = function` in the globals of a state, as the engine's own
/// registrations do it (e.g. `0x007874F0`).
///
/// # Safety
/// `state` must be a Lua state of the game, `name` a string that stays
/// alive.
unsafe fn set_global(state: *mut LuaState, name: *const c_char, function: LuaFn) {
    // SAFETY: three cdecl functions of the Lua 5.0 in the game.
    unsafe {
        let pushstring: unsafe extern "C" fn(*mut LuaState, *const c_char) =
            std::mem::transmute(addr::LUA_PUSHSTRING as usize);
        let pushcclosure: unsafe extern "C" fn(*mut LuaState, LuaFn, i32) =
            std::mem::transmute(addr::LUA_PUSHCCLOSURE as usize);
        let settable: unsafe extern "C" fn(*mut LuaState, i32) =
            std::mem::transmute(addr::LUA_SETTABLE as usize);
        pushstring(state, name);
        pushcclosure(state, function, 0);
        settable(state, addr::LUA_GLOBALSINDEX);
    }
}

/// Adds the binder to the LuaManager there is now, once per manager.
pub fn attach() {
    // SAFETY: the pointer of the LuaManager and its `AddBinder`
    // (`0x0043E2E0`, thiscall), called on the main thread.
    unsafe {
        let manager: u32 = read(addr::LUA_MANAGER);
        if manager == 0 || ADDED_TO.swap(manager, Ordering::Relaxed) == manager {
            return;
        }
        let add: unsafe extern "thiscall" fn(u32, *const Binder) =
            std::mem::transmute(addr::LUA_ADD_BINDER as usize);
        add(manager, &BINDER.0);
    }
    crate::log("Lua: the binder is added to the LuaManager");
}

/// What stands in for the constructor of the LuaManager at the call
/// `0x00681860`: the constructor, then the binder.
pub unsafe extern "thiscall" fn manager_ctor_hook(this: u32) -> u32 {
    // SAFETY: the function the call called before (checked bytes).
    let made = unsafe {
        let ctor: unsafe extern "thiscall" fn(u32) -> u32 =
            std::mem::transmute(addr::LUA_MANAGER_CTOR as usize);
        ctor(this)
    };
    let _ = std::panic::catch_unwind(|| {
        // A new manager: the binder is not in its list.
        ADDED_TO.store(0, Ordering::Relaxed);
        attach();
    });
    made
}

/// Registers a function of a mod.
pub fn register(name: *const c_char, function: LuaFn) -> Result<String, i32> {
    if name.is_null() {
        return Err(abi::ERR_ARGUMENT);
    }
    // SAFETY: a zero-terminated name the mod keeps alive.
    let text = unsafe { CStr::from_ptr(name) };
    if text.is_empty() {
        return Err(abi::ERR_ARGUMENT);
    }
    let mut functions = FUNCTIONS.lock().map_err(|_| abi::ERR_ENGINE)?;
    // SAFETY: names registered before, alive by the same rule.
    if functions
        .0
        .iter()
        .any(|(n, _)| unsafe { CStr::from_ptr(*n) } == text)
    {
        return Err(abi::ERR_TAKEN);
    }
    if functions.0.len() >= 1024 {
        return Err(abi::ERR_FULL);
    }
    functions.0.push((name, function));
    drop(functions);
    // States that exist already (a function registered after the start).
    // SAFETY: the LuaManager and its two states, `+0x18` and `+0x1C`.
    unsafe {
        let manager: u32 = read(addr::LUA_MANAGER);
        if manager != 0 && ADDED_TO.load(Ordering::Relaxed) == manager {
            for at in [0x18, 0x1C] {
                let state: u32 = read(manager + at);
                if state != 0 {
                    set_global(state as *mut LuaState, name, function);
                }
            }
        }
    }
    Ok(text.to_string_lossy().into_owned())
}

/// Number of registered functions.
pub fn count() -> usize {
    FUNCTIONS.lock().map_or(0, |f| f.0.len())
}

// --- The arguments and results of a running function -------------------------------

/// The value of argument `index` (from 1) of the running function:
/// `{type, value}` at `base + 8 × (index − 1)`, below the top.
unsafe fn value(state: *mut LuaState, index: i32) -> Option<(i32, u32)> {
    if index < 1 {
        return None;
    }
    // SAFETY: `lua_State`: top at `+8`, base at `+0xC` (as `0x00625740`
    // and `0x00625B60` read them).
    unsafe {
        let l = state as u32;
        let at = read::<u32>(l + addr::LUA_BASE) + 8 * (index as u32 - 1);
        (at < read::<u32>(l + addr::LUA_TOP)).then(|| (read(at), read(at + 4)))
    }
}

pub unsafe extern "C" fn gettop(state: *mut LuaState) -> i32 {
    // SAFETY: as `value`.
    unsafe {
        let l = state as u32;
        ((read::<u32>(l + addr::LUA_TOP) - read::<u32>(l + addr::LUA_BASE)) / 8) as i32
    }
}

pub unsafe extern "C" fn kind(state: *mut LuaState, index: i32) -> i32 {
    // SAFETY: as `value`.
    unsafe { value(state, index) }.map_or(abi::LUA_TNONE, |(t, _)| t)
}

pub unsafe extern "C" fn touserdata(state: *mut LuaState, index: i32) -> *mut c_void {
    // SAFETY: as `value`; a full userdata is a header of 0x10 bytes and
    // then the block (`0x00625B60`).
    match unsafe { value(state, index) } {
        Some((abi::LUA_TLIGHTUSERDATA, p)) => p as *mut c_void,
        Some((abi::LUA_TUSERDATA, p)) => (p + 0x10) as *mut c_void,
        _ => std::ptr::null_mut(),
    }
}

macro_rules! forward {
    ($name:ident, $address:expr, ($($arg:ident : $ty:ty),*) $(-> $ret:ty)?) => {
        pub unsafe extern "C" fn $name(state: *mut LuaState $(, $arg: $ty)*) $(-> $ret)? {
            // SAFETY: a cdecl function of the Lua 5.0 in the game, checked
            // by the edition check; positive indices only are promised.
            unsafe {
                let f: unsafe extern "C" fn(*mut LuaState $(, $ty)*) $(-> $ret)? =
                    std::mem::transmute($address as usize);
                f(state $(, $arg)*)
            }
        }
    };
}

forward!(tonumber, addr::LUA_TONUMBER, (index: i32) -> f32);
forward!(toboolean, addr::LUA_TOBOOLEAN, (index: i32) -> i32);
forward!(tostring, addr::LUA_TOSTRING, (index: i32) -> *const c_char);
forward!(pushnil, addr::LUA_PUSHNIL, ());
forward!(pushnumber, addr::LUA_PUSHNUMBER, (value: f32));
forward!(pushboolean, addr::LUA_PUSHBOOLEAN, (value: i32));
forward!(pushstring, addr::LUA_PUSHSTRING, (text: *const c_char));
forward!(pushlightuserdata, addr::LUA_PUSHLIGHTUSERDATA, (pointer: *mut c_void));
