//! `doctor_example.asi`: an example of a native mod (stage 80).
//!
//! What it adds to the game through `doctor_platform.asi`:
//!
//! - the class `DoctorCounter` of SimGroup objects (described for the
//!   toolkit in `classes.toml`): an object counts the message of attribute
//!   0 and, every `limit`-th time (attribute 2), sends the message of
//!   attribute 1 with itself as the data; the message of attribute 3 sets
//!   the count back to 0;
//! - the Lua functions `DoctorCounterInfo()` — objects alive, messages
//!   counted, messages sent, pings heard, messages swallowed, calls of
//!   `EclipseLog` seen; `DoctorSend(name [, seconds [, about]])`, which
//!   sends a message, at once or after `seconds` of game time, with the
//!   last counter as its data when `about` is true; `DoctorObject()` and
//!   `DoctorGuid(object)` — the last counter as a light userdata and the
//!   GUID of such an object; `DoctorSelfTest()`, which tries the rest of
//!   the platform's functions on the last counter and returns the results;
//! - a listener of the message `doctor_example_ping`, which names the
//!   object the message is about;
//! - a filter that swallows the message `doctor_example_blocked`;
//! - a hook on the game's Lua function `EclipseLog` (`0x0043E5C0`), which
//!   only counts its calls.
//!
//! `build.sh` builds the DLL and the data of the mod; what was checked in
//! the game is in `docs/analysis/game-bench.md` §43.

use std::ffi::{CString, c_void};
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, Ordering::Relaxed};

use doctor_sdk::abi::{self, Api, LuaState, Message};
use doctor_sdk::{Attr, Object, ObjectClass, Platform};

static OBJECTS: AtomicU32 = AtomicU32::new(0);
static COUNTED: AtomicU32 = AtomicU32::new(0);
static SENT: AtomicU32 = AtomicU32::new(0);
static PINGS: AtomicU32 = AtomicU32::new(0);
static SWALLOWED: AtomicU32 = AtomicU32::new(0);
static ECLIPSE_CALLS: AtomicU32 = AtomicU32::new(0);

/// The counter loaded last (null when it is gone), for the Lua functions.
static LAST: AtomicPtr<abi::Object> = AtomicPtr::new(std::ptr::null_mut());
/// The subscription of the ping listener.
static PING_LISTENER: AtomicI32 = AtomicI32::new(0);

const PING: u32 = abi::hash(b"doctor_example_ping");
const BLOCKED: u32 = abi::hash(b"doctor_example_blocked");

/// The game's Lua function `EclipseLog`: `mov eax, 1; ret`.
const ECLIPSE_LOG: u32 = 0x0043_E5C0;
/// What runs the five bytes the hook replaced.
static ECLIPSE_LOG_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// The data of one `DoctorCounter` object.
#[derive(Default)]
struct Counter {
    count_on: u32,
    send: u32,
    reset_on: u32,
    limit: i32,
    count: i32,
    /// The object, once it is loaded.
    me: Option<Object>,
}

impl ObjectClass for Counter {
    const NAME: &'static std::ffi::CStr = c"DoctorCounter";

    fn attribute(&mut self, p: &Platform, object: Object, attr: &Attr) {
        match attr.index {
            // A message to listen to: hash the name and subscribe.
            0 | 3 => {
                let msg = attr.text().map_or(0, |name| p.hash(name));
                if msg != 0 {
                    p.listen(object, msg);
                }
                if attr.index == 0 {
                    self.count_on = msg;
                } else {
                    self.reset_on = msg;
                }
            }
            1 => self.send = attr.text().map_or(0, |name| p.hash(name)),
            // An attribute the object does not store comes as 0.
            2 => self.limit = attr.int().max(1),
            _ => {}
        }
    }

    fn loaded(&mut self, p: &Platform, object: Object) {
        self.me = Some(object);
        LAST.store(object.0, Relaxed);
        OBJECTS.fetch_add(1, Relaxed);
        let guid = p.guid(object).unwrap_or_default();
        let text = format!(
            "DoctorCounter {:02x}{:02x}{:02x}{:02x}…: counts {:08x}, sends {:08x} every {} time(s)",
            guid[0], guid[1], guid[2], guid[3], self.count_on, self.send, self.limit
        );
        p.log(&CString::new(text).unwrap_or_default());
    }

    fn message(&mut self, p: &Platform, object: Object, msg: u32, _word: u32) {
        if msg == self.reset_on {
            self.count = 0;
        } else if msg == self.count_on {
            COUNTED.fetch_add(1, Relaxed);
            self.count += 1;
            if self.count >= self.limit {
                self.count = 0;
                SENT.fetch_add(1, Relaxed);
                if self.send != 0 {
                    p.send_about(self.send, object);
                }
            }
        }
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        if let Some(me) = self.me {
            OBJECTS.fetch_sub(1, Relaxed);
            let _ = LAST.compare_exchange(me.0, std::ptr::null_mut(), Relaxed, Relaxed);
        }
    }
}

/// `DoctorCounterInfo()`: six numbers.
unsafe extern "C" fn lua_info(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    // SAFETY: the state this function was called with.
    let lua = unsafe { p.lua(state) };
    let values = [
        &OBJECTS,
        &COUNTED,
        &SENT,
        &PINGS,
        &SWALLOWED,
        &ECLIPSE_CALLS,
    ];
    for v in values {
        lua.push_number(v.load(Relaxed) as f32);
    }
    values.len() as i32
}

/// `DoctorSend(name [, seconds [, about]])`: sends a message — at once
/// (the result: whether somebody listened) or after `seconds` of game time
/// (the result: whether the game took it), with the last counter as the
/// data of the message when `about` is true.
unsafe extern "C" fn lua_send(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    // SAFETY: the state this function was called with.
    let lua = unsafe { p.lua(state) };
    let done = match lua.text(1) {
        Some(name) if lua.kind(1) == abi::LUA_TSTRING => {
            let msg = p.hash(name);
            let last = LAST.load(Relaxed);
            let about = (lua.boolean(3) && !last.is_null()).then_some(Object(last));
            if lua.kind(2) == abi::LUA_TNUMBER && lua.number(2) > 0.0 {
                p.send_later(msg, lua.number(2), about) == abi::OK
            } else if let Some(about) = about {
                p.send_about(msg, about)
            } else {
                p.send(msg)
            }
        }
        _ => false,
    };
    lua.push_boolean(done);
    1
}

/// `DoctorObject()`: the last counter as a light userdata, or nil.
unsafe extern "C" fn lua_object(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    let last = LAST.load(Relaxed);
    // SAFETY: the state this function was called with.
    unsafe {
        if last.is_null() {
            (p.api().lua_pushnil)(state);
        } else {
            (p.api().lua_pushlightuserdata)(state, last.cast());
        }
    }
    1
}

/// `DoctorGuid(object)`: the GUID of the object `DoctorObject` gave, as 32
/// hexadecimal digits; nil for anything else.
unsafe extern "C" fn lua_guid(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    // SAFETY: the state this function was called with.
    let lua = unsafe { p.lua(state) };
    // SAFETY: as above.
    let object = unsafe { (p.api().lua_touserdata)(state, 1) };
    // Only the pointer this mod handed out is an object.
    let guid = (!object.is_null() && object.cast() == LAST.load(Relaxed))
        .then(|| p.guid(Object(object.cast())))
        .flatten();
    match guid {
        Some(g) => {
            let text: String = g.iter().map(|b| format!("{b:02x}")).collect();
            lua.push_text(&CString::new(text).unwrap_or_default());
        }
        None => lua.push_nil(),
    }
    1
}

/// `DoctorSelfTest()`: the functions of the platform the rest of the mod
/// does not use, tried on the last counter; returns one line of results.
unsafe extern "C" fn lua_self_test(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    // SAFETY: the state this function was called with.
    let lua = unsafe { p.lua(state) };
    let last = LAST.load(Relaxed);
    if last.is_null() {
        lua.push_nil();
        return 1;
    }
    let object = Object(last);
    let api = p.api();
    let count = abi::hash(b"doctor_example_count");
    let counted = || COUNTED.load(Relaxed);
    // SAFETY: functions of the table on an object of this mod's class.
    let text = unsafe {
        // Delivery to one object: only this counter counts.
        let before = counted();
        let to = (api.send_to)(count, 0, last, 0);
        let direct = counted() - before;
        // A listener switched off hears nothing; switched on, it hears.
        let off = (api.object_enable)(last, 0);
        let before = counted();
        (api.send_to)(count, 0, last, 0);
        let while_off = counted() - before;
        let on = (api.object_enable)(last, 1);
        // Without the subscription a broadcast does not reach the object.
        let unlisten = (api.object_unlisten)(last, count);
        let before = counted();
        p.send(count);
        let unsubscribed = counted() - before;
        let listen = p.listen(object, count);
        // The data of the object and what is not an object of the platform.
        let data = !(api.object_data)(last).is_null();
        let foreign = (api.object_listen)(state.cast(), count, abi::PRIORITY_DEFAULT);
        // The listener of the ping: off, a ping, on again.
        let pings = PINGS.load(Relaxed);
        let unsubscribe = (api.unsubscribe)(PING_LISTENER.load(Relaxed));
        p.send(PING);
        let missed = PINGS.load(Relaxed) == pings;
        let again = p.subscribe(PING, on_ping, std::ptr::null_mut());
        PING_LISTENER.store(again, Relaxed);
        format!(
            "send_to {to}: counted {direct}; enable(0) {off}: counted {while_off}; enable(1) {on}; \
             unlisten {unlisten}: others counted {unsubscribed}; listen {listen}; data {data}; \
             listen of a foreign pointer {foreign}; unsubscribe {unsubscribe}: ping missed {missed}; \
             subscribe {}",
            again > 0
        )
    };
    lua.push_text(&CString::new(text).unwrap_or_default());
    1
}

unsafe extern "C" fn on_ping(_: *mut c_void, _msg: u32, word: u32) {
    let n = PINGS.fetch_add(1, Relaxed) + 1;
    if let Some(p) = Platform::get() {
        // The data of the message points to the object that sent it.
        let about = match p.data_object(word).and_then(|o| p.guid(o)) {
            Some(g) => format!("about {:02x}{:02x}{:02x}{:02x}…", g[0], g[1], g[2], g[3]),
            None => "without an object".to_owned(),
        };
        let text = format!("doctor_example_ping heard ({n}) {about}");
        p.log(&CString::new(text).unwrap_or_default());
    }
}

unsafe extern "C" fn filter(_: *mut c_void, message: *mut Message, _force: u32) -> u32 {
    // SAFETY: the message the bus is sending.
    if unsafe { (*message).hash } == BLOCKED {
        SWALLOWED.fetch_add(1, Relaxed);
        return 1;
    }
    0
}

/// Stands in for `EclipseLog`: counts, then runs the original.
unsafe extern "C" fn eclipse_log(state: *mut LuaState) -> i32 {
    ECLIPSE_CALLS.fetch_add(1, Relaxed);
    let original = ECLIPSE_LOG_ORIGINAL.load(Relaxed);
    if original.is_null() {
        return 1;
    }
    // SAFETY: the trampoline of the platform: the replaced `mov eax, 1`
    // and a jump back to the `ret` of the function, a cdecl Lua function.
    unsafe {
        let f: unsafe extern "C" fn(*mut LuaState) -> i32 = std::mem::transmute(original);
        f(state)
    }
}

/// The entry of the mod.
///
/// # Safety
/// Called by `doctor_platform.asi` with its table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn doctor_mod_init(api: *const Api) -> i32 {
    // SAFETY: the table of the platform.
    let Some(p) = (unsafe { Platform::new(api) }) else {
        return abi::ERR_STATE;
    };
    let class = p.register_class::<Counter>();
    if class != abi::OK {
        return class;
    }
    let lua = [
        p.register_lua(c"DoctorCounterInfo", lua_info),
        p.register_lua(c"DoctorSend", lua_send),
        p.register_lua(c"DoctorObject", lua_object),
        p.register_lua(c"DoctorGuid", lua_guid),
        p.register_lua(c"DoctorSelfTest", lua_self_test),
    ];
    let ping = p.subscribe(PING, on_ping, std::ptr::null_mut());
    PING_LISTENER.store(ping, Relaxed);
    // SAFETY: functions of the table, with a function and a place of the
    // game this mod knows.
    let (filtered, taken, hooked) = unsafe {
        let api = p.api();
        let filtered = (api.add_filter)(filter, std::ptr::null_mut());
        // The entry of the message bus belongs to doctor_live: refused.
        let taken = (api.hook_check)(0x0040_8A00, 5);
        let mut original = std::ptr::null();
        let hooked = (api.hook_jump)(ECLIPSE_LOG, 5, eclipse_log as *const c_void, &mut original);
        ECLIPSE_LOG_ORIGINAL.store(original.cast_mut(), Relaxed);
        (filtered, taken, hooked)
    };
    let text = format!(
        "doctor_example on {}: class {class}, Lua {lua:?}, listener {ping}, filter {filtered}, \
         hook of the bus entry {taken} (refused is right), hook of EclipseLog {hooked}",
        p.edition().to_string_lossy()
    );
    p.log(&CString::new(text).unwrap_or_default());
    abi::OK
}
