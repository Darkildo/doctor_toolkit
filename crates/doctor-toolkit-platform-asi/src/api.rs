//! The table of functions a mod gets ([`doctor_sdk::abi::Api`]).
//!
//! Every function catches panics and checks what it is given; what it does
//! is in the modules it calls.

use std::ffi::{CStr, c_char, c_void};

use doctor_sdk::abi::{self, Api, Class, FilterFn, LuaFn, MessageFn, Object};
use doctor_toolkit_live::edition;
use doctor_toolkit_live::platform::addr;

use crate::{bus, classes, hooks, lua};

/// Runs `f`; a panic becomes `ERR_ENGINE`.
fn guard(f: impl FnOnce() -> i32 + std::panic::UnwindSafe) -> i32 {
    std::panic::catch_unwind(f).unwrap_or(abi::ERR_ENGINE)
}

unsafe extern "C" fn edition_name() -> *const c_char {
    // The name of the build, zero-terminated.
    static NAME: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    NAME.get_or_init(|| std::ffi::CString::new(edition::NAME).unwrap_or_default())
        .as_ptr()
}

unsafe extern "C" fn log(text: *const c_char) {
    if text.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: a zero-terminated string of the mod.
        let text = unsafe { CStr::from_ptr(text) };
        crate::log(&format!("mod: {}", text.to_string_lossy()));
    });
}

unsafe extern "C" fn hash(name: *const c_char) -> u32 {
    if name.is_null() {
        return 0;
    }
    // SAFETY: a zero-terminated string of the mod.
    abi::hash(unsafe { CStr::from_ptr(name) }.to_bytes())
}

unsafe extern "C" fn register_class(class: *const Class) -> i32 {
    let class = class as usize;
    guard(move || {
        if !crate::in_init() {
            crate::log("register_class refused: only from doctor_mod_init");
            return abi::ERR_STATE;
        }
        match classes::register(class as *const Class) {
            Ok(what) => {
                crate::log(&format!("class {what} registered"));
                abi::OK
            }
            Err(code) => {
                crate::log(&format!("register_class failed: {code}"));
                code
            }
        }
    })
}

unsafe extern "C" fn object_data(object: *mut Object) -> *mut c_void {
    classes::data(object as u32) as *mut c_void
}

unsafe extern "C" fn object_guid(object: *mut Object, out: *mut u8) -> i32 {
    if object.is_null() || out.is_null() {
        return abi::ERR_ARGUMENT;
    }
    // SAFETY: an object of the game: its GUID is 16 bytes at `+0x20`.
    unsafe {
        std::ptr::copy_nonoverlapping((object as u32 + addr::OBJECT_GUID) as *const u8, out, 16);
    }
    abi::OK
}

unsafe extern "C" fn object_listen(object: *mut Object, msg: u32, priority: u32) -> i32 {
    let object = object as u32;
    guard(move || {
        if !bus::ready() {
            return abi::ERR_STATE;
        }
        classes::listen(object, msg, priority)
    })
}

unsafe extern "C" fn object_unlisten(object: *mut Object, msg: u32) -> i32 {
    let object = object as u32;
    guard(move || classes::unlisten(object, msg))
}

unsafe extern "C" fn object_enable(object: *mut Object, on: u32) -> i32 {
    if classes::data(object as u32) == 0 {
        return abi::ERR_STATE;
    }
    // SAFETY: an object of a mod class; bit 0 of the listener's flags is
    // what the engine's own switch methods change (`0x0046C720`,
    // `0x0046C740`).
    unsafe {
        let flags = (object as u32 + addr::OBJECT_LISTENER + addr::LISTENER_FLAGS) as *mut u32;
        if on != 0 {
            *flags |= 1;
        } else {
            *flags &= !1;
        }
    }
    abi::OK
}

unsafe extern "C" fn send(msg: u32, word: u32, force: u32) -> i32 {
    guard(move || bus::send(msg, word, force))
}

unsafe extern "C" fn send_object(msg: u32, about: *mut Object, force: u32) -> i32 {
    let about = about as u32;
    guard(move || bus::send_object(msg, about, force))
}

unsafe extern "C" fn send_to(msg: u32, word: u32, to: *mut Object, force: u32) -> i32 {
    let to = to as u32;
    guard(move || bus::send_to(msg, word, to, force))
}

unsafe extern "C" fn send_later(msg: u32, seconds: f32, about: *mut Object) -> i32 {
    let about = about as u32;
    guard(move || bus::send_later(msg, seconds, about))
}

unsafe extern "C" fn data_object(word: u32) -> *mut Object {
    bus::data_object(word) as *mut Object
}

unsafe extern "C" fn subscribe(
    msg: u32,
    priority: u32,
    listener: MessageFn,
    user: *mut c_void,
) -> i32 {
    let user = user as usize;
    guard(move || bus::subscribe_function(msg, priority, listener, user as *mut c_void))
}

unsafe extern "C" fn unsubscribe(id: i32) -> i32 {
    guard(move || bus::unsubscribe_function(id))
}

unsafe extern "C" fn add_filter(filter: FilterFn, user: *mut c_void) -> i32 {
    let user = user as usize;
    guard(move || {
        if !crate::in_init() {
            crate::log("add_filter refused: only from doctor_mod_init");
            return abi::ERR_STATE;
        }
        bus::add_filter(filter, user as *mut c_void)
    })
}

unsafe extern "C" fn register_lua(name: *const c_char, function: LuaFn) -> i32 {
    let name = name as usize;
    guard(
        move || match lua::register(name as *const c_char, function) {
            Ok(name) => {
                crate::log(&format!("Lua function {name} registered"));
                abi::OK
            }
            Err(code) => {
                crate::log(&format!("register_lua failed: {code}"));
                code
            }
        },
    )
}

unsafe extern "C" fn hook_check(address: u32, len: u32) -> i32 {
    guard(move || hooks::check(address, len))
}

unsafe extern "C" fn hook_jump(
    address: u32,
    len: u32,
    detour: *const c_void,
    trampoline: *mut *const c_void,
) -> i32 {
    let (detour, trampoline) = (detour as usize, trampoline as usize);
    guard(move || {
        hooks::jump(
            address,
            len,
            detour as *const c_void,
            trampoline as *mut *const c_void,
        )
    })
}

unsafe extern "C" fn hook_call(
    address: u32,
    target: *const c_void,
    original: *mut *const c_void,
) -> i32 {
    let (target, original) = (target as usize, original as usize);
    guard(move || {
        hooks::call(
            address,
            target as *const c_void,
            original as *mut *const c_void,
        )
    })
}

/// The table.
pub static API: Api = Api {
    size: std::mem::size_of::<Api>() as u32,
    abi: abi::ABI_VERSION,
    edition: edition_name,
    log,
    hash,
    register_class,
    object_data,
    object_guid,
    object_listen,
    object_unlisten,
    object_enable,
    send,
    send_object,
    send_to,
    send_later,
    data_object,
    subscribe,
    unsubscribe,
    add_filter,
    register_lua,
    lua_gettop: lua::gettop,
    lua_type: lua::kind,
    lua_tonumber: lua::tonumber,
    lua_toboolean: lua::toboolean,
    lua_tostring: lua::tostring,
    lua_touserdata: lua::touserdata,
    lua_pushnil: lua::pushnil,
    lua_pushnumber: lua::pushnumber,
    lua_pushboolean: lua::pushboolean,
    lua_pushstring: lua::pushstring,
    lua_pushlightuserdata: lua::pushlightuserdata,
    hook_check,
    hook_jump,
    hook_call,
};
