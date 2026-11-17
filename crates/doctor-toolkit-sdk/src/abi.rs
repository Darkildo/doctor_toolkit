//! The binary interface between `doctor_platform.asi` and a native mod.
//!
//! Everything here is `repr(C)` and `cdecl`, and `include/doctor_sdk.h`
//! declares the same things for C; a test compares the two. What is promised
//! to stay and what is not: `docs/analysis/asi-platform.md` §7.
//!
//! - The table [`Api`] only grows: a new function goes to its end and
//!   [`Api::size`] grows with it; a mod built against an older table keeps
//!   working, a mod that wants a newer function checks `size` first.
//! - [`ABI_VERSION`] changes only when something existing changes its
//!   meaning or layout; the platform then refuses mods of the other number
//!   and a mod must refuse a platform of another number.

use core::ffi::{c_char, c_void};

/// Version of this interface.
pub const ABI_VERSION: u32 = 1;

/// Name of the function a mod exports; the platform calls it once, on the
/// main thread, after the engine has registered its own classes.
pub const MOD_ENTRY: &str = "doctor_mod_init";

/// Done.
pub const OK: i32 = 0;
/// A bad argument: a null pointer, an empty name, a length out of range.
pub const ERR_ARGUMENT: i32 = -1;
/// Not now: the part of the game the call needs does not exist yet (or the
/// object is not one of the platform's).
pub const ERR_STATE: i32 = -2;
/// The name or the place is taken: a class or a Lua function of that name
/// exists, another module patches the place.
pub const ERR_TAKEN: i32 = -3;
/// A table of the platform is full.
pub const ERR_FULL: i32 = -4;
/// The game refused or the memory could not be written.
pub const ERR_ENGINE: i32 = -5;

/// Lua types as `lua_type` returns them (Lua 5.0).
pub const LUA_TNONE: i32 = -1;
pub const LUA_TNIL: i32 = 0;
pub const LUA_TBOOLEAN: i32 = 1;
pub const LUA_TLIGHTUSERDATA: i32 = 2;
pub const LUA_TNUMBER: i32 = 3;
pub const LUA_TSTRING: i32 = 4;
pub const LUA_TTABLE: i32 = 5;
pub const LUA_TFUNCTION: i32 = 6;
pub const LUA_TUSERDATA: i32 = 7;

/// The priority nearly every subscription of the game has; a listener with
/// a higher number is called earlier.
pub const PRIORITY_DEFAULT: u32 = 0x8000;

/// An object of the game (a `Base` of the engine); opaque.
#[repr(C)]
pub struct Object {
    _opaque: [u8; 0],
}

/// A Lua state of the game; opaque.
#[repr(C)]
pub struct LuaState {
    _opaque: [u8; 0],
}

/// One attribute of the packet of a class, as the reader of an object gets
/// it. The reader is given every attribute of the packet in order, also the
/// ones the object does not store: those come with `present == 0` and a
/// zero value, exactly as the readers of the engine see them.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Attr {
    /// Number of the attribute in the packet.
    pub index: u32,
    /// 1 when the object stores a value, 0 when it does not.
    pub present: u32,
    /// The value word: an integer, the bits of a float, a hash.
    pub raw: u32,
    /// For a string, a GUID, a vector or a matrix: where the data is (text
    /// ends with a zero byte); null when the attribute is not stored. Valid
    /// only during the call.
    pub ptr: *const c_void,
}

/// A message on the bus, as the engine keeps it while sending.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Message {
    /// `sdbm(lower(name))`.
    pub hash: u32,
    /// The data word: 0 or a pointer to an object of data the sender built.
    pub data: u32,
    /// Set by the bus while the message is being delivered.
    pub delivering: u8,
}

/// A new class of SimGroup objects. The structure and the name must stay
/// alive for as long as the game runs.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Class {
    /// `sizeof(DoctorClass)` of the mod.
    pub size: u32,
    /// Name of the class; its attributes are the packet of the same name.
    pub name: *const c_char,
    /// Bytes of the mod's own data in every object, zeroed at creation.
    pub data_size: u32,
    /// The object exists (the constructor of `Base` has run), nothing is
    /// read yet.
    pub create: Option<unsafe extern "C" fn(object: *mut Object, data: *mut c_void)>,
    /// One attribute of the packet of the class.
    pub attribute:
        Option<unsafe extern "C" fn(object: *mut Object, data: *mut c_void, attr: *const Attr)>,
    /// Every attribute has been given.
    pub loaded: Option<unsafe extern "C" fn(object: *mut Object, data: *mut c_void)>,
    /// A message the object listens to.
    pub message:
        Option<unsafe extern "C" fn(object: *mut Object, data: *mut c_void, msg: u32, word: u32)>,
    /// The object is being destroyed; its subscriptions are dropped after
    /// this call.
    pub destroy: Option<unsafe extern "C" fn(object: *mut Object, data: *mut c_void)>,
}

/// A listener of a mod that is not an object.
pub type MessageFn = unsafe extern "C" fn(user: *mut c_void, msg: u32, word: u32);
/// A filter of sends to all listeners: returns 0 to let the message go on,
/// anything else to swallow it (the send then returns `false`). It may
/// change `message.data`.
pub type FilterFn =
    unsafe extern "C" fn(user: *mut c_void, message: *mut Message, force: u32) -> u32;
/// A function of Lua: returns the number of its results.
pub type LuaFn = unsafe extern "C" fn(state: *mut LuaState) -> i32;

macro_rules! api {
    ($( $(#[$doc:meta])* $name:ident : fn($($arg:ident : $ty:ty),*) $(-> $ret:ty)? ;)*) => {
        /// The table of functions the platform gives to a mod. All of them
        /// are to be called on the main thread of the game (from the
        /// callbacks the platform makes), unless said otherwise.
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct Api {
            /// Size of the table of this platform, bytes.
            pub size: u32,
            /// [`ABI_VERSION`] of this platform.
            pub abi: u32,
            $( $(#[$doc])* pub $name: unsafe extern "C" fn($($arg: $ty),*) $(-> $ret)?, )*
        }

        /// Names of the functions of [`Api`], in order (after `size` and
        /// `abi`).
        pub const API_FUNCTIONS: &[&str] = &[$(stringify!($name)),*];
    };
}

api! {
    /// Name of the build of the game the platform runs in.
    edition: fn() -> *const c_char;
    /// One line into `doctor_platform.log`. Any thread.
    log: fn(text: *const c_char);
    /// `sdbm(lower(name))`: the hash of a message, a class, a name. Any
    /// thread.
    hash: fn(name: *const c_char) -> u32;

    /// Registers a class the way the engine registers its own. Only from
    /// `doctor_mod_init`.
    register_class: fn(class: *const Class) -> i32;
    /// The mod's data of an object of one of its classes; null for any
    /// other object.
    object_data: fn(object: *mut Object) -> *mut c_void;
    /// The 16 bytes of the GUID of an object.
    object_guid: fn(object: *mut Object, out: *mut u8) -> i32;
    /// Subscribes an object of a mod class to a message; the subscription
    /// ends with the object.
    object_listen: fn(object: *mut Object, msg: u32, priority: u32) -> i32;
    /// Ends a subscription made by `object_listen`.
    object_unlisten: fn(object: *mut Object, msg: u32) -> i32;
    /// Switches the listener of an object on (1) or off (0): a listener
    /// that is off gets only forced messages.
    object_enable: fn(object: *mut Object, on: u32) -> i32;

    /// Sends a message to every listener; 1 when somebody listened.
    send: fn(msg: u32, word: u32, force: u32) -> i32;
    /// The same with data that points to an object, as the objects of the
    /// game send "myself".
    send_object: fn(msg: u32, about: *mut Object, force: u32) -> i32;
    /// Delivers a message to one object only.
    send_to: fn(msg: u32, word: u32, to: *mut Object, force: u32) -> i32;
    /// Sends a message to every listener after `seconds` of game time;
    /// `about` (may be null) becomes its data.
    send_later: fn(msg: u32, seconds: f32, about: *mut Object) -> i32;
    /// The object a data word points to, when it is the engine's pointer to
    /// an object; null otherwise.
    data_object: fn(word: u32) -> *mut Object;
    /// Subscribes a function; returns an identifier above 0 or an error.
    subscribe: fn(msg: u32, priority: u32, listener: MessageFn, user: *mut c_void) -> i32;
    /// Ends a subscription made by `subscribe`.
    unsubscribe: fn(id: i32) -> i32;
    /// Adds a filter of sends to all listeners.
    add_filter: fn(filter: FilterFn, user: *mut c_void) -> i32;

    /// Registers a global function of Lua in every state of the game, also
    /// in the ones made later. The name must stay alive.
    register_lua: fn(name: *const c_char, function: LuaFn) -> i32;
    /// Number of arguments of the running function.
    lua_gettop: fn(state: *mut LuaState) -> i32;
    /// Type of argument `index` (from 1).
    lua_type: fn(state: *mut LuaState, index: i32) -> i32;
    /// Argument `index` as a number (numbers of the game's Lua are floats).
    lua_tonumber: fn(state: *mut LuaState, index: i32) -> f32;
    lua_toboolean: fn(state: *mut LuaState, index: i32) -> i32;
    /// Argument `index` as text; null when it is neither a string nor a
    /// number.
    lua_tostring: fn(state: *mut LuaState, index: i32) -> *const c_char;
    /// Argument `index` as a pointer (light or full userdata).
    lua_touserdata: fn(state: *mut LuaState, index: i32) -> *mut c_void;
    lua_pushnil: fn(state: *mut LuaState);
    lua_pushnumber: fn(state: *mut LuaState, value: f32);
    lua_pushboolean: fn(state: *mut LuaState, value: i32);
    lua_pushstring: fn(state: *mut LuaState, text: *const c_char);
    lua_pushlightuserdata: fn(state: *mut LuaState, pointer: *mut c_void);

    /// Whether a mod may patch `len` bytes of code at `address`: `OK`, or
    /// why not. Any thread.
    hook_check: fn(address: u32, len: u32) -> i32;
    /// Replaces `len` (5 to 16) bytes of whole instructions at `address`
    /// with a jump to `detour` and gives a trampoline that runs them and
    /// goes on after them. The instructions must not use relative
    /// addresses, except that the first may be a `call` or `jmp` with a
    /// 32-bit offset.
    hook_jump: fn(address: u32, len: u32, detour: *const c_void, trampoline: *mut *const c_void) -> i32;
    /// Points the `call` at `address` (5 bytes, 32-bit offset) to `target`
    /// and gives what it called.
    hook_call: fn(address: u32, target: *const c_void, original: *mut *const c_void) -> i32;
}

/// The entry of a mod: `int doctor_mod_init(const DoctorApi*)`. Returns
/// [`OK`] or an error; the platform writes it into its log.
pub type ModInit = unsafe extern "C" fn(api: *const Api) -> i32;

/// `sdbm(lower(name))` — the hash the game uses for the names of messages,
/// classes and attributes values.
pub const fn hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    let mut i = 0;
    while i < name.len() {
        let c = name[i].to_ascii_lowercase();
        h = (c as u32)
            .wrapping_add(h << 6)
            .wrapping_add(h << 16)
            .wrapping_sub(h);
        i += 1;
    }
    h
}
