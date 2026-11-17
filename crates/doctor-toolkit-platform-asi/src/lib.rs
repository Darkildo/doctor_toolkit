//! `doctor_platform.asi`: the platform of native mods of The Godfather II
//! (stage 80).
//!
//! The ASI loader (`dinput8.dll`) loads it from the game folder's
//! `scripts/`, together with the native mods — other `.asi` files that
//! export `doctor_mod_init`. The module needs none of the toolkit's other
//! modules and gets in the way of none: the places it patches are in the
//! registry all of them are checked against
//! ([`doctor_toolkit_live::platform::CLAIMS`]).
//!
//! On load it checks the build ([`doctor_toolkit_live::platform::CHECKS`]);
//! on another build it installs nothing, calls no mod and says why in its
//! log. On PC 1.0.789.0 it replaces two calls:
//!
//! 1. the call of the engine's class registration in the start of the
//!    application (`0x0068AB55`): after the engine has registered its 299
//!    classes, the module finds the loaded mods and calls their
//!    `doctor_mod_init` with the table of functions ([`api`]) — new
//!    classes ([`classes`]), Lua functions ([`lua`]), messages ([`bus`])
//!    and hooks ([`hooks`]);
//! 2. the call of the LuaManager's constructor (`0x00681860`): the module
//!    adds its binder, which registers the Lua functions of the mods in
//!    every state.
//!
//! A third patch, the filter of the message bus (`0x00408A1A`), is made
//! only when a mod asks for a filter.
//!
//! Its log is `scripts/doctor_platform.log`. Nothing unwinds into the game
//! or into a mod: every entry point catches panics.

#![allow(clippy::missing_safety_doc)]

mod api;
mod bus;
mod classes;
mod hooks;
mod lua;
mod win;

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use doctor_sdk::abi;
use doctor_toolkit_live::edition;
use doctor_toolkit_live::platform::{self, addr};

/// Version written to the log.
pub const MODULE_VERSION: &str = concat!("doctor_platform ", env!("CARGO_PKG_VERSION"));

static MODULE: AtomicU32 = AtomicU32::new(0);
static LOADED_TICK: AtomicU32 = AtomicU32::new(0);
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// The build is the known one and the two calls are replaced.
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// `doctor_mod_init` of the mods is running.
static IN_INIT: AtomicBool = AtomicBool::new(false);

/// Folder of the module (`scripts/` of the game).
fn module_dir() -> PathBuf {
    let path = PathBuf::from(win::module_path(MODULE.load(Ordering::Relaxed)));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// One line in `doctor_platform.log`, with the seconds since the module
/// loaded.
pub(crate) fn log(line: &str) {
    let Ok(mut file) = LOG.lock() else {
        return;
    };
    if file.is_none() {
        *file = std::fs::File::create(module_dir().join(platform::LOG_FILE)).ok();
    }
    if let Some(f) = file.as_mut() {
        // SAFETY: a plain Win32 call.
        let ms = unsafe { win::GetTickCount() }.wrapping_sub(LOADED_TICK.load(Ordering::Relaxed));
        let _ = writeln!(f, "[{:5}.{:03}] {line}", ms / 1000, ms % 1000);
        let _ = f.flush();
    }
}

/// Whether the mods are being initialised now.
pub(crate) fn in_init() -> bool {
    IN_INIT.load(Ordering::Relaxed)
}

/// The mods: every loaded module that exports `doctor_mod_init`, by the
/// name of its file.
fn mods() -> Vec<(String, abi::ModInit)> {
    let me = MODULE.load(Ordering::Relaxed);
    let mut out = Vec::new();
    for base in win::modules() {
        if base == me {
            continue;
        }
        // SAFETY: a module of this process; the name is zero-terminated.
        let entry = unsafe {
            win::GetProcAddress(
                base as usize as *mut c_void,
                c"doctor_mod_init".as_ptr().cast(),
            )
        };
        if entry.is_null() {
            continue;
        }
        let path = win::module_path(base);
        let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
        // SAFETY: the export every mod declares as `ModInit`.
        out.push((name, unsafe {
            std::mem::transmute::<*mut c_void, abi::ModInit>(entry)
        }));
    }
    out.sort_by_key(|m| m.0.to_ascii_lowercase());
    out
}

/// Calls `doctor_mod_init` of every mod. On the main thread, after the
/// engine has registered its classes.
fn init_mods() {
    let mods = mods();
    if mods.is_empty() {
        log("no native mods (no loaded module exports doctor_mod_init)");
        return;
    }
    log(&format!(
        "message table {}, LuaManager {}",
        if bus::ready() {
            "exists"
        } else {
            "does not exist yet"
        },
        // SAFETY: a pointer in the data of the game.
        if unsafe { win::read::<u32>(addr::LUA_MANAGER) } != 0 {
            "exists"
        } else {
            "does not exist yet"
        }
    ));
    IN_INIT.store(true, Ordering::Relaxed);
    for (name, init) in mods {
        let before = (classes::count(), lua::count(), hooks::count());
        // SAFETY: the entry of a mod, called with the table of the platform.
        let result = unsafe { init(&api::API) };
        log(&format!(
            "mod {name}: doctor_mod_init returned {result}; classes +{}, Lua functions +{}, hooks +{}",
            classes::count() - before.0,
            lua::count() - before.1,
            hooks::count() - before.2
        ));
    }
    IN_INIT.store(false, Ordering::Relaxed);
}

/// What stands in for the registration of the engine's classes at the call
/// `0x0068AB55`: the registration, then the mods.
extern "C" fn start_hook() {
    // SAFETY: the function the call called before (checked bytes), cdecl
    // without arguments.
    unsafe {
        let register: unsafe extern "C" fn() = std::mem::transmute(addr::START_CALLEE as usize);
        register();
    }
    let _ = std::panic::catch_unwind(|| {
        init_mods();
        lua::attach();
    });
}

/// Points a `call rel32` of the game to `target`.
///
/// # Safety
/// `site` must be a call checked by the edition check.
unsafe fn redirect(site: u32, target: u32) -> bool {
    let rel = target.wrapping_sub(site + 5);
    // SAFETY: only the 4-byte target of the call changes.
    unsafe { win::patch(site + 1, &rel.to_le_bytes()) }
}

/// Checks the build, then replaces the two calls.
fn install() -> Result<(), String> {
    platform::check(&mut |va, buf| {
        // The image of the game is mapped from 0x00400000 for its
        // SizeOfImage; every checked place lies inside it.
        let end = edition::IMAGE_BASE + edition::SIZE_OF_IMAGE;
        if va < edition::IMAGE_BASE || va as u64 + buf.len() as u64 > end as u64 {
            return false;
        }
        // SAFETY: inside the mapped image.
        unsafe {
            std::ptr::copy_nonoverlapping(va as usize as *const u8, buf.as_mut_ptr(), buf.len())
        };
        true
    })
    .map_err(|e| e.to_string())?;
    classes::prepare();
    bus::prepare();
    let start = start_hook as extern "C" fn() as usize as u32;
    let lua = lua::manager_ctor_hook as unsafe extern "thiscall" fn(u32) -> u32 as usize as u32;
    // SAFETY: two calls checked above; from DllMain, before the game's
    // code runs.
    unsafe {
        if !redirect(addr::START_CALL, start) {
            return Err(format!("could not patch {:#010x}", addr::START_CALL));
        }
        if !redirect(addr::LUA_MANAGER_CALL, lua) {
            return Err(format!("could not patch {:#010x}", addr::LUA_MANAGER_CALL));
        }
    }
    INSTALLED.store(true, Ordering::Relaxed);
    Ok(())
}

/// The table of the platform, for a mod that was not loaded at the start
/// (null on another build).
#[unsafe(no_mangle)]
pub extern "C" fn doctor_platform_api() -> *const abi::Api {
    if INSTALLED.load(Ordering::Relaxed) {
        &api::API
    } else {
        std::ptr::null()
    }
}

/// Entry point of the DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(module: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == win::DLL_PROCESS_ATTACH {
        let _ = std::panic::catch_unwind(|| {
            MODULE.store(module as u32, Ordering::Relaxed);
            // SAFETY: plain Win32 calls allowed under the loader lock.
            unsafe {
                win::DisableThreadLibraryCalls(module);
                LOADED_TICK.store(win::GetTickCount(), Ordering::Relaxed);
            }
            let installed = install();
            log(&format!(
                "{MODULE_VERSION} loaded, ABI {}",
                abi::ABI_VERSION
            ));
            match installed {
                Ok(()) => log(&format!(
                    "edition {}; hooks: start of the application at {:#010x}, LuaManager at {:#010x}",
                    edition::NAME,
                    addr::START_CALL,
                    addr::LUA_MANAGER_CALL
                )),
                Err(why) => log(&format!("{why}; nothing installed, no mod is called")),
            }
        });
    }
    1
}
