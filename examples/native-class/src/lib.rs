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
//!   only counts its calls;
//! - a listener of the frame tick (stage 115), which counts the frames by
//!   the mode of the tick and writes a line to the platform's log when the
//!   mode changes (the game pauses or goes on), and a listener of the
//!   Direct3D device, which counts and logs its losses and resets.
//!   `DoctorCounterInfo()` returns these four counts after the six above;
//! - an overlay (stages 116, 118): a panel in the style of the game's menus
//!   drawn with `doctor_overlay` — a title, three items, the second chosen,
//!   a line of Cyrillic; hidden at the start, `DoctorOverlay(on)` shows or
//!   hides it and returns the frames drawn and the size of the back buffer;
//! - a sketch of a mod's menu on the input gate (stage 117): the key `~`
//!   (DirectInput `DIK_GRAVE`, whatever the layout; reserved, so the game
//!   never sees it) shows the square and
//!   closes the game's input; `~` again or `Escape` hides it and opens the
//!   input. `DoctorInput()` returns the serial of the poll, the captures
//!   held, whether the "menu" is open and whether `W` is pressed.
//!
//! `build.sh` builds the DLL and the data of the mod; what was checked in
//! the game is in `docs/analysis/game-bench.md` §43.

use std::ffi::{CString, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, Ordering::Relaxed};

use doctor_overlay::{Color, Face, Painter};

use doctor_sdk::abi::{self, Api, LuaState, Message};
use doctor_sdk::{Attr, Object, ObjectClass, Platform};

static OBJECTS: AtomicU32 = AtomicU32::new(0);
static COUNTED: AtomicU32 = AtomicU32::new(0);
static SENT: AtomicU32 = AtomicU32::new(0);
static PINGS: AtomicU32 = AtomicU32::new(0);
static SWALLOWED: AtomicU32 = AtomicU32::new(0);
static ECLIPSE_CALLS: AtomicU32 = AtomicU32::new(0);
/// Frames of the tick while the game runs and while it is paused.
static TICKS_RUNNING: AtomicU32 = AtomicU32::new(0);
static TICKS_PAUSED: AtomicU32 = AtomicU32::new(0);
/// The mode of the last tick.
static TICK_MODE: AtomicU32 = AtomicU32::new(0);
/// Losses and resets of the device.
static DEVICE_LOST: AtomicU32 = AtomicU32::new(0);
static DEVICE_RESET: AtomicU32 = AtomicU32::new(0);
/// The overlay: its subscription, frames drawn, the last size.
static OVERLAY: AtomicI32 = AtomicI32::new(0);
static OVERLAY_FRAMES: AtomicU32 = AtomicU32::new(0);
static OVERLAY_SIZE: AtomicU32 = AtomicU32::new(0);
/// The "menu" of the input gate: open, and the keys of the last tick.
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
static MENU_KEYS: AtomicU32 = AtomicU32::new(0);
/// `DIK_W`.
const KEY_W: u8 = 0x11;

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
        &TICKS_RUNNING,
        &TICKS_PAUSED,
        &DEVICE_LOST,
        &DEVICE_RESET,
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
/// Opens or closes the "menu": the square and the capture of the input.
fn set_menu(p: &Platform, open: bool) {
    if MENU_OPEN.swap(open, Relaxed) == open {
        return;
    }
    p.overlay_show(OVERLAY.load(Relaxed), open);
    let held = p.capture_input(open);
    let text = format!(
        "doctor_example: menu {}, captures held {held}",
        if open { "open" } else { "closed" }
    );
    p.log(&CString::new(text).unwrap_or_default());
}

/// The keys of the "menu", on every tick: `~` toggles, `Escape` closes.
fn menu_keys(p: &Platform) {
    let Some(input) = p.input() else {
        return;
    };
    let down = |k: u8| input.keys[k as usize] & 0x80 != 0;
    let now = u32::from(down(abi::KEY_GRAVE)) | (u32::from(down(abi::KEY_ESCAPE)) << 1);
    let before = MENU_KEYS.swap(now, Relaxed);
    let pressed = now & !before;
    if pressed & 1 != 0 {
        set_menu(p, !MENU_OPEN.load(Relaxed));
    } else if pressed & 2 != 0 && MENU_OPEN.load(Relaxed) {
        set_menu(p, false);
    }
}

/// `DoctorInput()`: the serial of the poll, the captures held, whether the
/// "menu" is open, whether `W` is pressed.
unsafe extern "C" fn lua_input(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    let Some(input) = p.input() else {
        return 0;
    };
    // SAFETY: the state this function was called with.
    let lua = unsafe { p.lua(state) };
    lua.push_number(input.serial as f32);
    lua.push_number(input.captured as f32);
    lua.push_boolean(MENU_OPEN.load(Relaxed));
    lua.push_boolean(input.keys[KEY_W as usize] & 0x80 != 0);
    4
}

/// The frame tick: counts the frames and logs a change of the mode.
unsafe extern "C" fn on_tick(_: *mut c_void, mode: u32) {
    if let Some(p) = Platform::get() {
        menu_keys(&p);
    }
    match mode {
        abi::TICK_PAUSED => TICKS_PAUSED.fetch_add(1, Relaxed),
        _ => TICKS_RUNNING.fetch_add(1, Relaxed),
    };
    let before = TICK_MODE.swap(mode, Relaxed);
    if before != mode
        && let Some(p) = Platform::get()
    {
        let text = format!(
            "doctor_example: tick mode {before} -> {mode} after {} running and {} paused frames",
            TICKS_RUNNING.load(Relaxed),
            TICKS_PAUSED.load(Relaxed)
        );
        p.log(&CString::new(text).unwrap_or_default());
    }
}

/// The device: counts and logs its losses and resets.
unsafe extern "C" fn on_device(_: *mut c_void, event: u32) {
    let n = match event {
        abi::DEVICE_LOST => DEVICE_LOST.fetch_add(1, Relaxed) + 1,
        _ => DEVICE_RESET.fetch_add(1, Relaxed) + 1,
    };
    if let Some(p) = Platform::get() {
        let what = if event == abi::DEVICE_LOST {
            "lost"
        } else {
            "reset"
        };
        let text = format!("doctor_example: device {what} ({n})");
        p.log(&CString::new(text).unwrap_or_default());
    }
}

/// The painter of the overlay: the fonts and their texture.
static PAINTER: Mutex<Painter> = Mutex::new(Painter::new());

/// Colours of the game's menus (as the mod manager has them).
const COAL: Color = Color::rgb(0x15, 0x12, 0x0c);
const OXBLOOD: Color = Color::rgb(0x4c, 0x00, 0x00);
const MUSTARD: Color = Color::rgb(0xab, 0xa4, 0x08);
const BONE: Color = Color::rgb(0xe7, 0xdf, 0xcb);
const BONE_DIM: Color = Color::rgb(0xa9, 0x9d, 0x82);
const INK: Color = Color::rgb(0x1b, 0x16, 0x04);

/// The overlay: a panel in the style of the game's menus.
unsafe extern "C" fn on_overlay(_: *mut c_void, overlay: *const abi::Overlay) {
    // SAFETY: the platform gives a valid description for the call.
    let o = unsafe { &*overlay };
    OVERLAY_FRAMES.fetch_add(1, Relaxed);
    OVERLAY_SIZE.store((o.width << 16) | (o.height & 0xFFFF), Relaxed);
    let Ok(mut painter) = PAINTER.lock() else {
        return;
    };
    let Some(mut f) = painter.frame(o) else {
        return;
    };
    let s = f.scale;
    let (x, y, w) = (48.0 * s, 48.0 * s, 420.0 * s);
    let h = 300.0 * s;
    f.rect(x, y, w, h, COAL.alpha(235));
    f.frame_rect(x, y, w, h, 3.0 * s, OXBLOOD);
    f.text(
        Face::Title,
        x + 24.0 * s,
        y + 16.0 * s,
        40.0 * s,
        MUSTARD,
        "DOCTOR",
    );
    let items = ["God mode", "Money +$10 000", "Teleport to the waypoint"];
    let mut line = y + 80.0 * s;
    for (i, item) in items.iter().enumerate() {
        let lh = f.line_height(Face::Text, 26.0 * s);
        if i == 1 {
            f.rect(x + 12.0 * s, line, w - 24.0 * s, lh, MUSTARD);
            f.text(Face::Text, x + 24.0 * s, line, 26.0 * s, INK, item);
        } else {
            f.text(Face::Text, x + 24.0 * s, line, 26.0 * s, BONE, item);
        }
        line += lh + 6.0 * s;
    }
    f.text(
        Face::Text,
        x + 24.0 * s,
        line + 10.0 * s,
        22.0 * s,
        BONE_DIM,
        "Меню разработчика — «Ёлка», №1",
    );
}

/// `DoctorOverlay(on)`: shows or hides the square; returns the frames
/// drawn, the width and the height of the back buffer.
unsafe extern "C" fn lua_overlay(state: *mut LuaState) -> i32 {
    let Some(p) = Platform::get() else {
        return 0;
    };
    // SAFETY: the state this function was called with.
    let lua = unsafe { p.lua(state) };
    if lua.top() >= 1 {
        p.overlay_show(OVERLAY.load(Relaxed), lua.boolean(1));
    }
    let size = OVERLAY_SIZE.load(Relaxed);
    lua.push_number(OVERLAY_FRAMES.load(Relaxed) as f32);
    lua.push_number((size >> 16) as f32);
    lua.push_number((size & 0xFFFF) as f32);
    3
}

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
        p.register_lua(c"DoctorOverlay", lua_overlay),
        p.register_lua(c"DoctorInput", lua_input),
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
    let tick = p.tick_listen(on_tick, std::ptr::null_mut());
    let device = p.device_listen(on_device, std::ptr::null_mut());
    // `~` opens the "menu": the game never sees it.
    let reserved = p.reserve_key(abi::KEY_GRAVE, true);
    // The fonts are rasterised now, not in the first frame of the overlay.
    let fonts = PAINTER
        .lock()
        .map(|mut painter| painter.atlas().map(|a| (a.width, a.height)))
        .ok()
        .flatten();
    let overlay = p.overlay_listen(on_overlay, std::ptr::null_mut(), 0);
    if overlay > 0 {
        OVERLAY.store(overlay, Relaxed);
        p.overlay_show(overlay, false);
    }
    let text = format!(
        "doctor_example on {}: class {class}, Lua {lua:?}, listener {ping}, filter {filtered}, \
         hook of the bus entry {taken} (refused is right), hook of EclipseLog {hooked}, \
         tick {tick}, device {device}, overlay {overlay}, key ~ reserved {reserved}, \
         font atlas {fonts:?}",
        p.edition().to_string_lossy()
    );
    p.log(&CString::new(text).unwrap_or_default());
    abi::OK
}
