//! SDK of native mods of The Godfather II (PC 1.0.789.0), stage 80.
//!
//! A native mod is a 32-bit DLL with the extension `.asi` in the `scripts`
//! folder of the game, next to `doctor_platform.asi`. It exports
//! `doctor_mod_init`; the platform module calls it once, on the main thread
//! of the game, right after the engine has registered its own classes, and
//! gives it the table of functions [`abi::Api`]:
//!
//! - new classes of SimGroup objects, registered the way the engine
//!   registers its own: the game creates the objects from the SimGroups of
//!   a mod, they read their attributes, listen to messages and send them;
//! - new global functions of Lua;
//! - sending, listening to and filtering the messages of the bus;
//! - hooks in the code of the game that cannot collide with the hooks of
//!   the toolkit's own modules.
//!
//! [`abi`] is the raw interface, the same as the C header
//! `include/doctor_sdk.h`; the rest of the crate is a thin safe layer over
//! it. A mod in Rust:
//!
//! ```no_run
//! use doctor_sdk::{Attr, Object, ObjectClass, Platform};
//!
//! #[derive(Default)]
//! struct Counter {
//!     count: u32,
//! }
//!
//! impl ObjectClass for Counter {
//!     const NAME: &'static core::ffi::CStr = c"MyCounter";
//!     fn attribute(&mut self, p: &Platform, object: Object, attr: &Attr) {
//!         if attr.index == 0 {
//!             if let Some(name) = attr.text() {
//!                 p.listen(object, p.hash(name));
//!             }
//!         }
//!     }
//!     fn message(&mut self, _: &Platform, _: Object, _msg: u32, _word: u32) {
//!         self.count += 1;
//!     }
//! }
//!
//! #[unsafe(no_mangle)]
//! pub unsafe extern "C" fn doctor_mod_init(api: *const doctor_sdk::abi::Api) -> i32 {
//!     let Some(p) = (unsafe { Platform::new(api) }) else {
//!         return doctor_sdk::abi::ERR_STATE;
//!     };
//!     p.register_class::<Counter>()
//! }
//! ```
//!
//! What the platform promises to keep, the build of the game it works in
//! and how the engine does all this: `docs/analysis/asi-platform.md`.

pub mod abi;

use core::ffi::{CStr, c_char, c_void};
use core::sync::atomic::{AtomicPtr, Ordering};

pub use abi::{ABI_VERSION, Attr, LuaState, Message};

/// An object of the game, as the platform hands it to a mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object(pub *mut abi::Object);

impl Attr {
    /// The value as an integer.
    pub fn int(&self) -> i32 {
        self.raw as i32
    }

    /// The value as a float.
    pub fn float(&self) -> f32 {
        f32::from_bits(self.raw)
    }

    /// The value as text, when the attribute is stored and is a string.
    ///
    /// Only for attributes the mod knows to be strings: the pointer of a
    /// GUID or a vector is not text.
    pub fn text(&self) -> Option<&CStr> {
        if self.present == 0 || self.ptr.is_null() {
            return None;
        }
        // SAFETY: the platform gives a zero-terminated string of the record
        // for a stored string attribute; it lives during the call.
        let text = unsafe { CStr::from_ptr(self.ptr.cast()) };
        (!text.is_empty()).then_some(text)
    }
}

/// A class of a mod in Rust: one value of the type lives in every object.
pub trait ObjectClass: Default + 'static {
    /// Name of the class and of its packet of attributes.
    const NAME: &'static CStr;

    /// One attribute of the packet; called for every attribute in order,
    /// stored or not, when the game creates the object.
    fn attribute(&mut self, _platform: &Platform, _object: Object, _attr: &Attr) {}

    /// Every attribute has been given.
    fn loaded(&mut self, _platform: &Platform, _object: Object) {}

    /// A message the object listens to.
    fn message(&mut self, _platform: &Platform, _object: Object, _msg: u32, _word: u32) {}
}

/// The table of the platform, kept for the callbacks of the classes.
static API: AtomicPtr<abi::Api> = AtomicPtr::new(core::ptr::null_mut());

/// The platform, checked to speak this interface.
#[derive(Clone, Copy)]
pub struct Platform(&'static abi::Api);

impl Platform {
    /// Checks the table a mod got in `doctor_mod_init`: `None` when it is
    /// null, of another [`ABI_VERSION`] or shorter than this crate knows.
    ///
    /// # Safety
    /// `api` must be the pointer the platform gave (it stays valid while
    /// the game runs).
    pub unsafe fn new(api: *const abi::Api) -> Option<Self> {
        if api.is_null() {
            return None;
        }
        // SAFETY: `size` and `abi` are the first two words of every table.
        let (size, version) = unsafe { ((*api).size, (*api).abi) };
        if version != ABI_VERSION || (size as usize) < core::mem::size_of::<abi::Api>() {
            return None;
        }
        API.store(api.cast_mut(), Ordering::Relaxed);
        // SAFETY: checked above; the platform keeps the table for the
        // life of the process.
        Some(Self(unsafe { &*api }))
    }

    /// The platform, in a callback after [`Self::new`] succeeded.
    pub fn get() -> Option<Self> {
        let api = API.load(Ordering::Relaxed);
        // SAFETY: stored by `new` after its checks.
        (!api.is_null()).then(|| Self(unsafe { &*api }))
    }

    /// The raw table.
    pub fn api(&self) -> &'static abi::Api {
        self.0
    }

    /// Name of the build of the game.
    pub fn edition(&self) -> &'static CStr {
        // SAFETY: the platform returns a static string.
        unsafe { CStr::from_ptr((self.0.edition)()) }
    }

    /// One line into `doctor_platform.log`.
    pub fn log(&self, text: &CStr) {
        // SAFETY: a zero-terminated string.
        unsafe { (self.0.log)(text.as_ptr()) }
    }

    /// `sdbm(lower(name))`.
    pub fn hash(&self, name: &CStr) -> u32 {
        abi::hash(name.to_bytes())
    }

    /// Registers the class `T`; only from `doctor_mod_init`.
    pub fn register_class<T: ObjectClass>(&self) -> i32 {
        unsafe extern "C" fn create<T: ObjectClass>(_: *mut abi::Object, data: *mut c_void) {
            // SAFETY: `data_size` bytes of the object, aligned on 16.
            unsafe { data.cast::<T>().write(T::default()) }
        }
        unsafe extern "C" fn attribute<T: ObjectClass>(
            object: *mut abi::Object,
            data: *mut c_void,
            attr: *const Attr,
        ) {
            if let Some(p) = Platform::get() {
                // SAFETY: written by `create`; the attribute lives during the call.
                unsafe { (*data.cast::<T>()).attribute(&p, Object(object), &*attr) }
            }
        }
        unsafe extern "C" fn loaded<T: ObjectClass>(object: *mut abi::Object, data: *mut c_void) {
            if let Some(p) = Platform::get() {
                // SAFETY: written by `create`.
                unsafe { (*data.cast::<T>()).loaded(&p, Object(object)) }
            }
        }
        unsafe extern "C" fn message<T: ObjectClass>(
            object: *mut abi::Object,
            data: *mut c_void,
            msg: u32,
            word: u32,
        ) {
            if let Some(p) = Platform::get() {
                // SAFETY: written by `create`.
                unsafe { (*data.cast::<T>()).message(&p, Object(object), msg, word) }
            }
        }
        unsafe extern "C" fn destroy<T: ObjectClass>(_: *mut abi::Object, data: *mut c_void) {
            // SAFETY: written by `create`, dropped once.
            unsafe { data.cast::<T>().drop_in_place() }
        }
        assert!(core::mem::align_of::<T>() <= 16);
        let class = Box::leak(Box::new(abi::Class {
            size: core::mem::size_of::<abi::Class>() as u32,
            name: T::NAME.as_ptr(),
            data_size: core::mem::size_of::<T>() as u32,
            create: Some(create::<T>),
            attribute: Some(attribute::<T>),
            loaded: Some(loaded::<T>),
            message: Some(message::<T>),
            destroy: Some(destroy::<T>),
        }));
        // SAFETY: a leaked descriptor with a static name.
        unsafe { (self.0.register_class)(class) }
    }

    /// The GUID of an object.
    pub fn guid(&self, object: Object) -> Option<[u8; 16]> {
        let mut out = [0u8; 16];
        // SAFETY: 16 bytes for the GUID.
        (unsafe { (self.0.object_guid)(object.0, out.as_mut_ptr()) } == abi::OK).then_some(out)
    }

    /// Subscribes an object of a mod class to a message with the usual
    /// priority.
    pub fn listen(&self, object: Object, msg: u32) -> i32 {
        // SAFETY: the platform checks the object.
        unsafe { (self.0.object_listen)(object.0, msg, abi::PRIORITY_DEFAULT) }
    }

    /// Ends a subscription of an object.
    pub fn unlisten(&self, object: Object, msg: u32) -> i32 {
        // SAFETY: the platform checks the object.
        unsafe { (self.0.object_unlisten)(object.0, msg) }
    }

    /// Switches the listener of an object on or off.
    pub fn enable(&self, object: Object, on: bool) -> i32 {
        // SAFETY: the platform checks the object.
        unsafe { (self.0.object_enable)(object.0, u32::from(on)) }
    }

    /// Sends a message without data to every listener; whether somebody
    /// listened.
    pub fn send(&self, msg: u32) -> bool {
        // SAFETY: plain values.
        unsafe { (self.0.send)(msg, 0, 0) == 1 }
    }

    /// Sends a message whose data points to `about`.
    pub fn send_about(&self, msg: u32, about: Object) -> bool {
        // SAFETY: the platform checks the pointer for null only; the object
        // is one the game gave.
        unsafe { (self.0.send_object)(msg, about.0, 0) == 1 }
    }

    /// Sends a message to every listener after `seconds` of game time.
    pub fn send_later(&self, msg: u32, seconds: f32, about: Option<Object>) -> i32 {
        let about = about.map_or(core::ptr::null_mut(), |o| o.0);
        // SAFETY: as `send_about`.
        unsafe { (self.0.send_later)(msg, seconds, about) }
    }

    /// The object a data word of a message points to.
    pub fn data_object(&self, word: u32) -> Option<Object> {
        // SAFETY: the platform reads the word only when it is readable.
        let object = unsafe { (self.0.data_object)(word) };
        (!object.is_null()).then_some(Object(object))
    }

    /// Subscribes a function to a message; an identifier above 0 or an
    /// error. `user` is only handed back to the listener.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn subscribe(&self, msg: u32, listener: abi::MessageFn, user: *mut c_void) -> i32 {
        // SAFETY: a function of the mod, called on the main thread.
        unsafe { (self.0.subscribe)(msg, abi::PRIORITY_DEFAULT, listener, user) }
    }

    /// Registers a global function of Lua.
    pub fn register_lua(&self, name: &'static CStr, function: abi::LuaFn) -> i32 {
        // SAFETY: a static name and a function of the mod.
        unsafe { (self.0.register_lua)(name.as_ptr(), function) }
    }

    /// Arguments of a running Lua function.
    ///
    /// # Safety
    /// `state` must be the state the function was called with.
    pub unsafe fn lua(&self, state: *mut LuaState) -> Lua<'_> {
        Lua { api: self.0, state }
    }
}

/// The arguments and results of a running Lua function.
pub struct Lua<'a> {
    api: &'a abi::Api,
    state: *mut LuaState,
}

impl Lua<'_> {
    /// Number of arguments.
    pub fn top(&self) -> i32 {
        // SAFETY: the state of the running function.
        unsafe { (self.api.lua_gettop)(self.state) }
    }

    /// Type of argument `index` (from 1), `abi::LUA_T…`.
    pub fn kind(&self, index: i32) -> i32 {
        // SAFETY: as above.
        unsafe { (self.api.lua_type)(self.state, index) }
    }

    pub fn number(&self, index: i32) -> f32 {
        // SAFETY: as above.
        unsafe { (self.api.lua_tonumber)(self.state, index) }
    }

    pub fn boolean(&self, index: i32) -> bool {
        // SAFETY: as above.
        unsafe { (self.api.lua_toboolean)(self.state, index) != 0 }
    }

    /// Argument `index` as text; the string lives while it is on the stack.
    pub fn text(&self, index: i32) -> Option<&CStr> {
        // SAFETY: as above.
        let p: *const c_char = unsafe { (self.api.lua_tostring)(self.state, index) };
        // SAFETY: Lua strings end with a zero byte.
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) })
    }

    pub fn push_nil(&self) {
        // SAFETY: as above.
        unsafe { (self.api.lua_pushnil)(self.state) }
    }

    pub fn push_number(&self, value: f32) {
        // SAFETY: as above.
        unsafe { (self.api.lua_pushnumber)(self.state, value) }
    }

    pub fn push_boolean(&self, value: bool) {
        // SAFETY: as above.
        unsafe { (self.api.lua_pushboolean)(self.state, i32::from(value)) }
    }

    pub fn push_text(&self, text: &CStr) {
        // SAFETY: Lua copies the string.
        unsafe { (self.api.lua_pushstring)(self.state, text.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::abi::*;

    const HEADER: &str = include_str!("../include/doctor_sdk.h");

    /// The text of the header without comments.
    fn code() -> String {
        let mut out = String::new();
        let mut rest = HEADER;
        while let Some(at) = rest.find("/*") {
            out.push_str(&rest[..at]);
            rest = &rest[at..];
            rest = &rest[rest.find("*/").expect("a closed comment") + 2..];
        }
        out.push_str(rest);
        out
    }

    fn define(name: &str) -> i64 {
        let code = code();
        let line = code
            .lines()
            .find(|l| {
                l.split_whitespace()
                    .collect::<Vec<_>>()
                    .starts_with(&["#define", name])
            })
            .unwrap_or_else(|| panic!("no #define {name}"));
        let value = line.split_whitespace().nth(2).unwrap();
        let value = value.trim_matches(|c| c == '(' || c == ')');
        match value.strip_prefix("0x") {
            Some(hex) => i64::from_str_radix(hex, 16).unwrap(),
            None => value.parse().unwrap(),
        }
    }

    /// Names of the function pointers of a struct of the header, in order.
    fn pointers(name: &str) -> Vec<String> {
        let code = code();
        let start = code
            .find(&format!("typedef struct {name} {{"))
            .unwrap_or_else(|| panic!("no struct {name}"));
        let body = &code[start..];
        let body = &body[..body.find(&format!("}} {name};")).unwrap()];
        body.split("(*")
            .skip(1)
            .map(|s| s[..s.find(')').unwrap()].trim().to_owned())
            .collect()
    }

    #[test]
    fn the_header_has_the_constants_of_the_crate() {
        assert_eq!(define("DOCTOR_ABI_VERSION"), i64::from(ABI_VERSION));
        for (name, value) in [
            ("DOCTOR_OK", OK),
            ("DOCTOR_ERR_ARGUMENT", ERR_ARGUMENT),
            ("DOCTOR_ERR_STATE", ERR_STATE),
            ("DOCTOR_ERR_TAKEN", ERR_TAKEN),
            ("DOCTOR_ERR_FULL", ERR_FULL),
            ("DOCTOR_ERR_ENGINE", ERR_ENGINE),
            ("DOCTOR_LUA_TNONE", LUA_TNONE),
            ("DOCTOR_LUA_TNIL", LUA_TNIL),
            ("DOCTOR_LUA_TBOOLEAN", LUA_TBOOLEAN),
            ("DOCTOR_LUA_TLIGHTUSERDATA", LUA_TLIGHTUSERDATA),
            ("DOCTOR_LUA_TNUMBER", LUA_TNUMBER),
            ("DOCTOR_LUA_TSTRING", LUA_TSTRING),
            ("DOCTOR_LUA_TTABLE", LUA_TTABLE),
            ("DOCTOR_LUA_TFUNCTION", LUA_TFUNCTION),
            ("DOCTOR_LUA_TUSERDATA", LUA_TUSERDATA),
        ] {
            assert_eq!(define(name), i64::from(value), "{name}");
        }
        assert_eq!(
            define("DOCTOR_PRIORITY_DEFAULT"),
            i64::from(PRIORITY_DEFAULT)
        );
        assert!(HEADER.contains(&format!("int {MOD_ENTRY}(const DoctorApi *api);")));
    }

    #[test]
    fn the_header_lists_the_functions_in_the_order_of_the_table() {
        assert_eq!(pointers("DoctorApi"), API_FUNCTIONS);
        assert_eq!(
            pointers("DoctorClass"),
            ["create", "attribute", "loaded", "message", "destroy"]
        );
    }

    /// The sizes the header asserts for a 32-bit build are the sizes of the
    /// Rust structures there: every field is 4 bytes.
    #[test]
    fn the_header_asserts_the_sizes_of_a_32_bit_build() {
        let api = 8 + 4 * API_FUNCTIONS.len();
        assert!(HEADER.contains(&format!("DOCTOR_STATIC_ASSERT(sizeof(DoctorApi) == {api}")));
        assert!(HEADER.contains("DOCTOR_STATIC_ASSERT(sizeof(DoctorClass) == 32"));
        assert!(HEADER.contains("DOCTOR_STATIC_ASSERT(sizeof(DoctorAttr) == 16"));
        assert!(HEADER.contains("DOCTOR_STATIC_ASSERT(sizeof(DoctorMessage) == 12"));
        if core::mem::size_of::<usize>() == 4 {
            assert_eq!(core::mem::size_of::<Api>(), api);
            assert_eq!(core::mem::size_of::<Class>(), 32);
            assert_eq!(core::mem::size_of::<Attr>(), 16);
        }
        assert_eq!(core::mem::size_of::<Message>(), 12);
    }

    #[test]
    fn the_hash_is_the_hash_of_the_game() {
        // Class hashes of the engine's registry.
        assert_eq!(hash(b"TriggerAuto"), 0xACBD_FE47);
        assert_eq!(hash(b"triggerauto"), 0xACBD_FE47);
        assert_eq!(hash(b"MultiManager"), 0x6DF5_0074);
        assert_eq!(hash(b""), 0);
    }
}
