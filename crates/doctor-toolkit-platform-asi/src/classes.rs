//! New classes of SimGroup objects.
//!
//! A class of a mod is registered with the engine's own function
//! (`0x004833A0`: hash, name, factory, size), so the game creates its
//! objects from a SimGroup exactly as it creates its own
//! (`0x00483410`): `factory(record, extra)`, then method `+4` of the
//! object — the reader — with the record.
//!
//! An object of a mod class is a `Base` of the engine (0x50 bytes, built by
//! its constructor `0x0046C590`) followed by a header of the platform and
//! the data of the mod:
//!
//! ```text
//! +0x00  Base: method table, …, +0x3C listener, +0x48 third table
//! +0x50  Header: mark, class number, subscriptions
//! +0x60  data of the mod (`DoctorClass.data_size` bytes, zeroed)
//! ```
//!
//! The three method tables are copies of the tables of `Base` with the
//! destructor, the reader and the message handler replaced.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use doctor_sdk::abi::{self, Attr, Class, Object};
use doctor_toolkit_live::platform::addr;

use crate::win::{read, write};

/// Most classes all mods together may register.
pub const MAX_CLASSES: usize = 64;
const HEADER: u32 = doctor_toolkit_live::platform::OBJECT_MARK_AT;
const DATA: u32 = HEADER + 0x10;
const MARK: u32 = doctor_toolkit_live::platform::OBJECT_MARK;

/// One registered class.
#[derive(Clone, Copy)]
struct Entry {
    hash: u32,
    class: Class,
    /// Size of an object: `Base`, the header, the data, rounded up to 16.
    size: u32,
}

// SAFETY: the pointers of a descriptor are to static data of a mod, used
// on the main thread of the game.
unsafe impl Send for Entry {}

static CLASSES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
/// Objects of mod classes alive now, and created since the start.
pub static ALIVE: AtomicU32 = AtomicU32::new(0);
pub static CREATED: AtomicU32 = AtomicU32::new(0);

/// A subscription of an object: the variable of the message (the engine
/// keeps its address) and the hash.
struct Subscription {
    msg: u32,
    variable: *mut [u32; 2],
}

/// The method tables of an object of a mod class; filled once from the
/// tables of `Base`.
static mut OBJECT_VTABLE: [u32; addr::BASE_VTABLE_LEN] = [0; addr::BASE_VTABLE_LEN];
static mut LISTENER_VTABLE: [u32; addr::LISTENER_VTABLE_LEN] = [0; addr::LISTENER_VTABLE_LEN];
static mut THIRD_VTABLE: [u32; 1] = [0];

fn entry(index: u32) -> Option<Entry> {
    CLASSES.lock().ok()?.get(index as usize).copied()
}

/// The class number of an object of a mod class.
fn class_of(object: u32) -> Option<u32> {
    if object < 0x1_0000 {
        return None;
    }
    // SAFETY: an object pointer the game or a mod gave; only objects whose
    // first word is our table are read further.
    unsafe {
        if read::<u32>(object) != (&raw const OBJECT_VTABLE) as u32
            || read::<u32>(object + HEADER) != MARK
        {
            return None;
        }
        Some(read(object + HEADER + 4))
    }
}

unsafe extern "thiscall" fn object_dtor(this: u32, flags: u32) -> u32 {
    let _ = std::panic::catch_unwind(|| {
        if let Some(e) = class_of(this).and_then(entry)
            && let Some(destroy) = e.class.destroy
        {
            // SAFETY: the callback of the class of this object.
            unsafe { destroy(this as *mut Object, (this + DATA) as *mut c_void) };
        }
        // SAFETY: the list was made by `listen` for this object.
        unsafe {
            let subs: *mut Vec<Subscription> = read(this + HEADER + 8);
            if !subs.is_null() {
                for s in Box::from_raw(subs).drain(..) {
                    drop_subscription(this, s);
                }
                write(this + HEADER + 8, 0u32);
            }
            write(this + HEADER, 0u32);
        }
        ALIVE.fetch_sub(1, Ordering::Relaxed);
    });
    // SAFETY: the deleting destructor of `Base`: it puts its own tables
    // back, drops what is left of the listener and frees the memory.
    unsafe {
        let base: unsafe extern "thiscall" fn(u32, u32) -> u32 =
            std::mem::transmute(addr::BASE_DTOR as usize);
        base(this, flags)
    }
}

unsafe extern "thiscall" fn listener_dtor(this: u32, flags: u32) -> u32 {
    // SAFETY: the listener of an object lies at `+0x3C` of it.
    unsafe { object_dtor(this - addr::OBJECT_LISTENER, flags) }
}

unsafe extern "thiscall" fn third_dtor(this: u32, flags: u32) -> u32 {
    // SAFETY: the third table of an object lies at `+0x48` of it.
    unsafe { object_dtor(this - addr::OBJECT_THIRD, flags) }
}

/// The reader: gives the mod every attribute of the packet of the class.
unsafe extern "thiscall" fn reader(this: u32, record: u32) {
    let _ = std::panic::catch_unwind(|| {
        let Some(e) = class_of(this).and_then(entry) else {
            return;
        };
        let object = this as *mut Object;
        let data = (this + DATA) as *mut c_void;
        if let Some(attribute) = e.class.attribute {
            // SAFETY: the record the engine created the object from.
            unsafe {
                packet(record, e.hash, &mut |attr| attribute(object, data, &attr));
            }
        }
        if let Some(loaded) = e.class.loaded {
            // SAFETY: the callback of the class of this object.
            unsafe { loaded(object, data) };
        }
    });
}

/// Walks the packet of class `class` in a record of an object, as the
/// iterator of the engine does (`0x0043AFF0` and the loop every reader
/// repeats).
///
/// A packed record (bit 0 of the byte `+0x1C`): `+0x1F` packets, their
/// offsets (each relative to itself) from `+0x2C + 4 × byte(+0x1E)`; a
/// packet is `{hash, u16 count, mask, values}` — a set bit of the mask is
/// an attribute that is not stored, the values of the stored ones follow
/// aligned on 4. A string, a GUID, a vector or a matrix is at the address
/// of its value word plus the word.
///
/// An unpacked record: a list from `+0x20` of `{offset of the next, tag,
/// value}`; the tag `0x80000000` starts the attributes of the class named
/// by the text at `+8`, `0x20000000` and `0x40000000` are skipped
/// (`0x0043B140`), any other tag is the number of an attribute.
///
/// # Safety
/// `record` must be a record of a SimGroup the game is creating an object
/// from.
pub unsafe fn packet(record: u32, class: u32, give: &mut dyn FnMut(Attr)) {
    // SAFETY: the layout the engine itself reads.
    unsafe {
        if read::<u8>(record + 0x1C) & 1 != 0 {
            let packets = read::<u8>(record + 0x1F) as u32;
            let table = record + 0x2C + 4 * read::<u8>(record + 0x1E) as u32;
            for i in 0..packets {
                let slot = table + 4 * i;
                let p = slot.wrapping_add(read::<u32>(slot));
                if read::<u32>(p) != class {
                    continue;
                }
                let count = read::<u16>(p + 4) as u32;
                let mask = p + 6;
                let mut values = (mask + count.div_ceil(8) + 3) & !3;
                for index in 0..count {
                    if read::<u8>(mask + index / 8) & (1 << (index % 8)) != 0 {
                        give(Attr {
                            index,
                            present: 0,
                            raw: 0,
                            ptr: std::ptr::null(),
                        });
                    } else {
                        let raw: u32 = read(values);
                        give(Attr {
                            index,
                            present: 1,
                            raw,
                            ptr: values.wrapping_add(raw) as *const c_void,
                        });
                        values += 4;
                    }
                }
                return;
            }
        } else {
            let mut at = record + 0x20;
            let mut mine = false;
            for _ in 0..4096 {
                let next: u32 = read(at);
                if next == 0 {
                    break;
                }
                let tag: u32 = read(at + 4);
                match tag {
                    0x2000_0000 | 0x4000_0000 => {}
                    0x8000_0000 => {
                        let name = CStr::from_ptr((at + 8) as *const c_char);
                        mine = abi::hash(name.to_bytes()) == class;
                    }
                    index if mine => give(Attr {
                        index,
                        present: 1,
                        raw: read(at + 8),
                        ptr: (at + 8) as *const c_void,
                    }),
                    _ => {}
                }
                at = at.wrapping_add(next);
            }
        }
    }
}

/// The handler of the listener of an object.
unsafe extern "thiscall" fn handler(listener: u32, message: u32) {
    let _ = std::panic::catch_unwind(|| {
        let this = listener - addr::OBJECT_LISTENER;
        if let Some(e) = class_of(this).and_then(entry)
            && let Some(on_message) = e.class.message
        {
            // SAFETY: the message the bus is delivering: `{hash, data}`.
            unsafe {
                on_message(
                    this as *mut Object,
                    (this + DATA) as *mut c_void,
                    read(message),
                    read(message + 4),
                );
            }
        }
    });
}

/// The factory of class number `I`: what the registry of the engine calls.
unsafe extern "C" fn factory<const I: usize>(record: u32, extra: u32) -> u32 {
    std::panic::catch_unwind(|| {
        let Some(e) = entry(I as u32) else {
            return 0;
        };
        // SAFETY: the calls the factories of the engine make (e.g.
        // `0x00684A50` of TriggerAuto): memory of the SimManager, the
        // constructor of the base class, then the tables of the class.
        unsafe {
            let alloc: unsafe extern "C" fn(u32, u32) -> u32 =
                std::mem::transmute(addr::OBJECT_ALLOC as usize);
            let this = alloc(e.size, extra);
            if this == 0 {
                return 0;
            }
            let base: unsafe extern "thiscall" fn(u32, u32) -> u32 =
                std::mem::transmute(addr::BASE_CTOR as usize);
            base(this, record);
            std::ptr::write_bytes((this + HEADER) as *mut u8, 0, (e.size - HEADER) as usize);
            write(this + HEADER, MARK);
            write(this + HEADER + 4, I as u32);
            write(this, (&raw const OBJECT_VTABLE) as u32);
            write(
                this + addr::OBJECT_LISTENER,
                (&raw const LISTENER_VTABLE) as u32,
            );
            write(this + addr::OBJECT_THIRD, (&raw const THIRD_VTABLE) as u32);
            ALIVE.fetch_add(1, Ordering::Relaxed);
            CREATED.fetch_add(1, Ordering::Relaxed);
            if let Some(create) = e.class.create {
                create(this as *mut Object, (this + DATA) as *mut c_void);
            }
            this
        }
    })
    .unwrap_or(0)
}

macro_rules! factories {
    ($($i:literal)*) => {
        [$(factory::<$i> as unsafe extern "C" fn(u32, u32) -> u32),*]
    };
}

static FACTORIES: [unsafe extern "C" fn(u32, u32) -> u32; MAX_CLASSES] = factories!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
    32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63
);

/// Copies the tables of `Base` and puts the module's methods into them.
/// Once, after the edition check.
pub fn prepare() {
    // SAFETY: the tables checked by the edition check; written once before
    // any object exists.
    unsafe {
        let object = &raw mut OBJECT_VTABLE;
        for i in 0..addr::BASE_VTABLE_LEN {
            (*object)[i] = read(addr::BASE_VTABLE + 4 * i as u32);
        }
        (*object)[0] = object_dtor as unsafe extern "thiscall" fn(u32, u32) -> u32 as usize as u32;
        (*object)[1] = reader as unsafe extern "thiscall" fn(u32, u32) as usize as u32;
        let listener = &raw mut LISTENER_VTABLE;
        for i in 0..addr::LISTENER_VTABLE_LEN {
            (*listener)[i] = read(addr::BASE_LISTENER_VTABLE + 4 * i as u32);
        }
        (*listener)[0] =
            listener_dtor as unsafe extern "thiscall" fn(u32, u32) -> u32 as usize as u32;
        (*listener)[1] = handler as unsafe extern "thiscall" fn(u32, u32) as usize as u32;
        let third = &raw mut THIRD_VTABLE;
        (*third)[0] = third_dtor as unsafe extern "thiscall" fn(u32, u32) -> u32 as usize as u32;
    }
}

/// Whether the registry of the engine has a class with this hash.
fn registered(hash: u32) -> bool {
    // SAFETY: the registry: a pointer to 16-byte records and their number.
    unsafe {
        let records: u32 = read(addr::CLASS_REGISTRY);
        let count: u32 = read(addr::CLASS_REGISTRY + 4);
        records != 0 && (0..count.min(4096)).any(|i| read::<u32>(records + 16 * i) == hash)
    }
}

/// Registers a class of a mod. Returns the name for the log.
pub fn register(class: *const Class) -> Result<String, i32> {
    if class.is_null() {
        return Err(abi::ERR_ARGUMENT);
    }
    // SAFETY: a descriptor of a mod; its size is checked before the rest
    // is read.
    let class = unsafe {
        if ((*class).size as usize) < std::mem::size_of::<Class>() {
            return Err(abi::ERR_ARGUMENT);
        }
        *class
    };
    if class.name.is_null() || class.data_size > 0x10_0000 {
        return Err(abi::ERR_ARGUMENT);
    }
    // SAFETY: a zero-terminated name the mod keeps alive.
    let name = unsafe { CStr::from_ptr(class.name) };
    if name.is_empty() {
        return Err(abi::ERR_ARGUMENT);
    }
    let hash = abi::hash(name.to_bytes());
    if registered(hash) {
        return Err(abi::ERR_TAKEN);
    }
    let mut classes = CLASSES.lock().map_err(|_| abi::ERR_ENGINE)?;
    if classes.len() >= MAX_CLASSES {
        return Err(abi::ERR_FULL);
    }
    let size = (DATA + class.data_size + 15) & !15;
    let index = classes.len();
    classes.push(Entry { hash, class, size });
    drop(classes);
    // SAFETY: the registration the engine uses for its own classes, called
    // on the main thread right after them.
    unsafe {
        let register: unsafe extern "C" fn(u32, *const c_char, u32, u32) =
            std::mem::transmute(addr::CLASS_REGISTER as usize);
        register(hash, class.name, FACTORIES[index] as usize as u32, size);
    }
    if !registered(hash) {
        return Err(abi::ERR_ENGINE);
    }
    Ok(format!(
        "{} (hash {hash:08x}, object of {size:#x} bytes)",
        name.to_string_lossy()
    ))
}

/// The mod's data of an object of a mod class.
pub fn data(object: u32) -> u32 {
    class_of(object).map_or(0, |_| object + DATA)
}

/// Subscribes an object of a mod class to a message.
pub fn listen(object: u32, msg: u32, priority: u32) -> i32 {
    if class_of(object).is_none() {
        return abi::ERR_STATE;
    }
    if msg == 0 || priority > 0xFFFF {
        return abi::ERR_ARGUMENT;
    }
    // SAFETY: the calls of the engine's readers: name the variable, then
    // subscribe the listener of the object to it.
    unsafe {
        let mut subs: *mut Vec<Subscription> = read(object + HEADER + 8);
        if subs.is_null() {
            subs = Box::into_raw(Box::default());
            write(object + HEADER + 8, subs as u32);
        }
        if (*subs).iter().any(|s| s.msg == msg) {
            return abi::OK;
        }
        let variable = Box::into_raw(Box::new([0u32; 2]));
        crate::bus::name(variable, msg);
        crate::bus::subscribe(object + addr::OBJECT_LISTENER, variable, priority);
        (*subs).push(Subscription { msg, variable });
    }
    abi::OK
}

/// Ends a subscription and frees its variable.
///
/// # Safety
/// `s` must be a subscription of `object`.
unsafe fn drop_subscription(object: u32, s: Subscription) {
    // SAFETY: the calls of the engine's destructors (e.g. `0x00477E50`).
    unsafe {
        crate::bus::unsubscribe(object + addr::OBJECT_LISTENER, s.variable);
        crate::bus::release(s.variable);
        drop(Box::from_raw(s.variable));
    }
}

/// Ends a subscription made by [`listen`].
pub fn unlisten(object: u32, msg: u32) -> i32 {
    if class_of(object).is_none() {
        return abi::ERR_STATE;
    }
    // SAFETY: the list made by `listen`.
    unsafe {
        let subs: *mut Vec<Subscription> = read(object + HEADER + 8);
        if subs.is_null() {
            return abi::ERR_ARGUMENT;
        }
        let Some(at) = (*subs).iter().position(|s| s.msg == msg) else {
            return abi::ERR_ARGUMENT;
        };
        let s = (*subs).swap_remove(at);
        drop_subscription(object, s);
    }
    abi::OK
}

/// Number of registered classes.
pub fn count() -> usize {
    CLASSES.lock().map_or(0, |c| c.len())
}
