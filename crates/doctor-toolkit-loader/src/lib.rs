//! The ASI loader of the toolkit (stage 81): `doctor_loader.dll`, which the
//! mod manager installs into the game folder as `dinput8.dll` when the
//! folder has no ASI loader and a mod with executable code needs one.
//!
//! `godfather2.exe` imports one function of `DINPUT8.dll`,
//! `DirectInput8Create`. Windows looks for the DLL in the folder of the
//! program first, so a `dinput8.dll` there is loaded instead of the
//! system's (Wine needs `WINEDLLOVERRIDES=dinput8=n,b` for that). This one
//!
//! - when it is loaded, loads every `*.asi` of the game folder, of
//!   `scripts\` and of `plugins\` (the three places Ultimate ASI Loader, the
//!   loader players of this game use, looks in), in the order of their
//!   names, each folder after the other;
//! - passes `DirectInput8Create` (and the four COM entries of the DLL) on to
//!   the `dinput8.dll` of the system folder.
//!
//! It patches nothing, reads no settings and writes no files. It is the
//! toolkit's own code, so the toolkit can ship it; a loader the player
//! already has is never replaced (`doctor_toolkit::mods::native`).
//!
//! The modules are loaded from `DllMain`, before the entry point of the
//! game runs: the modules of the toolkit patch the start of the application
//! from their own `DllMain` and must be there by then.
#![cfg(windows)]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

type Handle = *mut c_void;
type Hresult = i32;

const DLL_PROCESS_ATTACH: u32 = 1;
const MAX_PATH: usize = 260;
const INVALID_HANDLE_VALUE: isize = -1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// `E_FAIL`.
const E_FAIL: Hresult = 0x8000_4005_u32 as i32;
/// `CLASS_E_CLASSNOTAVAILABLE`.
const CLASS_E_CLASSNOTAVAILABLE: Hresult = 0x8004_0111_u32 as i32;

/// `WIN32_FIND_DATAW`.
#[repr(C)]
struct FindData {
    attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    file_size_high: u32,
    file_size_low: u32,
    reserved0: u32,
    reserved1: u32,
    file_name: [u16; MAX_PATH],
    alternate_file_name: [u16; 14],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn DisableThreadLibraryCalls(module: Handle) -> i32;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> isize;
    fn FindNextFileW(find: isize, data: *mut FindData) -> i32;
    fn FindClose(find: isize) -> i32;
}

/// A path as Windows takes it: UTF-16 with the final zero.
fn wide(text: &[u16], more: &str) -> Vec<u16> {
    let mut out = text.to_vec();
    out.extend(more.encode_utf16());
    out.push(0);
    out
}

/// The folder of the program (`godfather2.exe`), with the final `\`.
fn program_folder() -> Option<Vec<u16>> {
    let mut path = vec![0u16; 1024];
    // SAFETY: the buffer is as long as said.
    let len =
        unsafe { GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), path.len() as u32) }
            as usize;
    if len == 0 || len >= path.len() {
        return None;
    }
    let end = path[..len].iter().rposition(|&c| c == u16::from(b'\\'))?;
    path.truncate(end + 1);
    Some(path)
}

/// Loads every `*.asi` of `folder` (with the final `\`), in the order of
/// the names; returns how many were loaded.
fn load_folder(folder: &[u16]) -> usize {
    let pattern = wide(folder, "*.asi");
    // SAFETY: all-zero is a valid `FindData`.
    let mut data: FindData = unsafe { core::mem::zeroed() };
    // SAFETY: the pattern ends with zero, the data is ours.
    let find = unsafe { FindFirstFileW(pattern.as_ptr(), &mut data) };
    if find == INVALID_HANDLE_VALUE {
        return 0;
    }
    let mut names: Vec<Vec<u16>> = Vec::new();
    loop {
        let len = data
            .file_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(MAX_PATH);
        let name = &data.file_name[..len];
        // `*.asi` also matches `x.asi2` through the short names of old
        // file systems: the ending is checked again.
        let asi = name.len() > 4
            && String::from_utf16_lossy(&name[name.len() - 4..]).eq_ignore_ascii_case(".asi");
        if asi && data.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            names.push(name.to_vec());
        }
        // SAFETY: the handle of the search above.
        if unsafe { FindNextFileW(find, &mut data) } == 0 {
            break;
        }
    }
    // SAFETY: the handle of the search above.
    unsafe { FindClose(find) };
    names.sort_by_key(|name| String::from_utf16_lossy(name).to_ascii_lowercase());
    let mut loaded = 0;
    for name in &names {
        let mut path = folder.to_vec();
        path.extend_from_slice(name);
        path.push(0);
        // SAFETY: the path ends with zero. A module that does not load is
        // skipped: the others and the game go on.
        if !unsafe { LoadLibraryW(path.as_ptr()) }.is_null() {
            loaded += 1;
        }
    }
    loaded
}

/// How many modules the loader loaded.
static LOADED: AtomicUsize = AtomicUsize::new(0);

/// How many `.asi` modules were loaded: for a module that wants to know
/// that this loader is the one in the game.
#[unsafe(no_mangle)]
pub extern "C" fn doctor_loader_modules() -> u32 {
    LOADED.load(Ordering::Relaxed) as u32
}

fn load_modules() {
    let Some(folder) = program_folder() else {
        return;
    };
    let mut loaded = load_folder(&folder);
    for sub in ["scripts\\", "plugins\\"] {
        let mut path = folder.clone();
        path.extend(sub.encode_utf16());
        loaded += load_folder(&path);
    }
    LOADED.store(loaded, Ordering::Relaxed);
}

/// The `dinput8.dll` of the system folder, loaded at the first call.
fn system_dinput() -> Handle {
    static MODULE: AtomicUsize = AtomicUsize::new(0);
    let known = MODULE.load(Ordering::Acquire);
    if known != 0 {
        return known as Handle;
    }
    let mut folder = vec![0u16; MAX_PATH + 1];
    // SAFETY: the buffer is as long as said.
    let len = unsafe { GetSystemDirectoryW(folder.as_mut_ptr(), folder.len() as u32) } as usize;
    if len == 0 || len >= folder.len() {
        return core::ptr::null_mut();
    }
    let path = wide(&folder[..len], "\\dinput8.dll");
    // SAFETY: the path ends with zero. A full path: this DLL of the same
    // name in the game folder is not taken for it.
    let module = unsafe { LoadLibraryW(path.as_ptr()) };
    MODULE.store(module as usize, Ordering::Release);
    module
}

/// The function `name` (ending with zero) of the system's `dinput8.dll`.
fn system_function(name: &'static [u8]) -> *mut c_void {
    let module = system_dinput();
    if module.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: a loaded module and a name that ends with zero.
    unsafe { GetProcAddress(module, name.as_ptr()) }
}

/// `DirectInput8Create` of the system's DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectInput8Create(
    instance: Handle,
    version: u32,
    interface: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> Hresult {
    type Create = unsafe extern "system" fn(
        Handle,
        u32,
        *const c_void,
        *mut *mut c_void,
        *mut c_void,
    ) -> Hresult;
    let function = system_function(b"DirectInput8Create\0");
    if function.is_null() {
        return E_FAIL;
    }
    // SAFETY: the function of the system's DLL has this signature; the
    // arguments are the caller's.
    unsafe {
        let create: Create = core::mem::transmute(function);
        create(instance, version, interface, out, outer)
    }
}

/// `DllCanUnloadNow` of the system's DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllCanUnloadNow() -> Hresult {
    let function = system_function(b"DllCanUnloadNow\0");
    if function.is_null() {
        // S_FALSE: the modules this DLL loaded stay.
        return 1;
    }
    // SAFETY: the function of the system's DLL has this signature.
    unsafe {
        let call: unsafe extern "system" fn() -> Hresult = core::mem::transmute(function);
        call()
    }
}

/// `DllGetClassObject` of the system's DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const c_void,
    interface: *const c_void,
    out: *mut *mut c_void,
) -> Hresult {
    type Get = unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> Hresult;
    let function = system_function(b"DllGetClassObject\0");
    if function.is_null() {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    // SAFETY: the function of the system's DLL has this signature.
    unsafe {
        let get: Get = core::mem::transmute(function);
        get(class, interface, out)
    }
}

/// `DllRegisterServer`: this DLL registers nothing.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllRegisterServer() -> Hresult {
    E_FAIL
}

/// `DllUnregisterServer`: this DLL registers nothing.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllUnregisterServer() -> Hresult {
    E_FAIL
}

/// Entry point of the DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(module: Handle, reason: u32, _: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // SAFETY: a plain Win32 call allowed under the loader lock.
        unsafe { DisableThreadLibraryCalls(module) };
        load_modules();
    }
    1
}
