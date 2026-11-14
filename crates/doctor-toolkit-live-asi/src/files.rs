//! Files the game opens from elsewhere (stage 46,
//! [`doctor_toolkit_live::files`]).
//!
//! Every call of the game through the import slots of `CreateFileA`
//! (`0x00D55120`), `CreateFileW` (`0x00D55254`) and `GetFileAttributesA`
//! (`0x00D5528C`) goes to the hooks here: the module changes those
//! instructions ([`doctor_toolkit_live::addr::CREATE_FILE_A_CALLS`] and the
//! others), not the slots, which the ASI loader puts back after it loaded
//! the modules. A hook turns the path the game names into a key relative to
//! the game folder; when the configuration redirects that key, it opens (or
//! asks about) the target with `CreateFileW` (`GetFileAttributesW`) of the
//! module's own imports, else it calls what the slot holds then. Nothing is
//! written into the game folder.
//!
//! The table is made once, before the hooks go in, and only read after;
//! the counters are atomics. The hooks run on every thread that opens a
//! file and catch panics.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use doctor_toolkit_live::addr::{
    CREATE_FILE_A_CALLS, CREATE_FILE_A_LOADS, CREATE_FILE_W_CALLS, CREATE_FILE_W_LOADS,
    GET_FILE_ATTRIBUTES_A_CALLS, IAT_CREATE_FILE_A, IAT_CREATE_FILE_W, IAT_GET_FILE_ATTRIBUTES_A,
};
use doctor_toolkit_live::config::Config;
use doctor_toolkit_live::files::{FileUse, Files, MAX_LISTED, key_in};

use crate::win::{self, Handle};

struct Entry {
    /// The target as the configuration names it.
    to: String,
    /// The absolute target, NUL-terminated.
    wide: Vec<u16>,
    opens: AtomicU32,
    failed: AtomicU32,
    /// Order of the first open (0: not opened yet).
    first: AtomicU32,
}

struct Table {
    /// The game folder (of `godfather2.exe`).
    game_dir: String,
    map: HashMap<String, Entry>,
    log_all: bool,
}

static TABLE: OnceLock<Table> = OnceLock::new();
static HOOKED: AtomicBool = AtomicBool::new(false);
/// Opens of paths in the game folder that were not redirected.
static PASSED: AtomicU32 = AtomicU32::new(0);
static FIRSTS: AtomicU32 = AtomicU32::new(0);
/// Paths already written to the log (`files_log=1`).
static LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

type CreateFileA = extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, Handle) -> Handle;
type CreateFileW =
    extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, Handle) -> Handle;
type GetFileAttributesA = extern "system" fn(*const u8) -> u32;

/// What an import slot holds now: the function of `kernel32`, or the hook
/// of another mod (the ASI loader's file loader) with the same arguments.
fn slot(address: u32) -> usize {
    // SAFETY: an import slot checked by `edition::check`, a 4-byte aligned
    // pointer the loader filled.
    unsafe { std::ptr::read_volatile(address as usize as *const u32) as usize }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The folder of the game's executable.
fn game_dir() -> String {
    let mut buf = [0u16; 1024];
    // SAFETY: fills `buf` with the path of the executable.
    let n = unsafe { win::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), 1024) };
    let path = String::from_utf16_lossy(&buf[..n as usize]);
    match path.rfind(['\\', '/']) {
        Some(i) => path[..i].to_owned(),
        None => path,
    }
}

fn current_dir() -> String {
    let mut buf = [0u16; 1024];
    // SAFETY: fills `buf`; a longer folder gives 0 characters here.
    let n = unsafe { win::GetCurrentDirectoryW(1024, buf.as_mut_ptr()) } as usize;
    String::from_utf16_lossy(&buf[..n.min(1024)])
}

/// A C string of the ANSI code page.
///
/// # Safety
/// `text` is null or a NUL-terminated string.
unsafe fn ansi(text: *const u8) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut out = [0u16; 1024];
    // SAFETY: a NUL-terminated string (-1: up to its NUL, included).
    let n = unsafe { win::MultiByteToWideChar(0, 0, text, -1, out.as_mut_ptr(), 1024) };
    (n > 0).then(|| String::from_utf16_lossy(&out[..n as usize - 1]))
}

/// A NUL-terminated UTF-16 string.
///
/// # Safety
/// `text` is null or a NUL-terminated string.
unsafe fn utf16(text: *const u16) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut len = 0usize;
    // SAFETY: up to its NUL; paths longer than 32 767 are not paths.
    while len < 32_768 && unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` characters read above.
    Some(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(text, len)
    }))
}

/// The redirect of a path the game names, if any; counts the open. An
/// open that may write (`!reading`) is never redirected. `site` is the
/// patched instruction the game called from (for the log).
fn find(path: &str, api: &str, site: u32, reading: bool) -> Option<&'static Entry> {
    let table = TABLE.get()?;
    let key = key_in(path, &current_dir(), &table.game_dir)?;
    let entry = table.map.get(&key).filter(|_| reading);
    match entry {
        Some(e) => {
            if e.opens.fetch_add(1, Ordering::Relaxed) == 0 {
                e.first.store(
                    FIRSTS.fetch_add(1, Ordering::Relaxed) + 1,
                    Ordering::Relaxed,
                );
                crate::log(&format!("files: {api} from {site:#010x} {key} -> {}", e.to));
            }
        }
        None => {
            PASSED.fetch_add(1, Ordering::Relaxed);
            if table.log_all
                && let Ok(mut logged) = LOGGED.lock()
                && logged.get_or_insert_with(HashSet::new).insert(key.clone())
            {
                crate::log(&format!(
                    "files: {api} from {site:#010x} {key} ({path}){}",
                    if reading { "" } else { ", may write" }
                ));
            }
        }
    }
    entry
}

/// Whether an open only reads an existing file: only those are redirected
/// (a target may be an original of the game folder under another name,
/// the `.viv` archives shifted by one; the game writes its own files
/// where it always did).
fn reads_only(access: u32, disposition: u32) -> bool {
    const WRITING: u32 = 0x4000_0000 // GENERIC_WRITE
        | 0x1000_0000 // GENERIC_ALL
        | 0x0001_0000 // DELETE
        | 0x0000_0002 // FILE_WRITE_DATA
        | 0x0000_0004 // FILE_APPEND_DATA
        | 0x0000_0100; // FILE_WRITE_ATTRIBUTES
    const OPEN_EXISTING: u32 = 3;
    access & WRITING == 0 && disposition == OPEN_EXISTING
}

fn failed(entry: &Entry) {
    if entry.failed.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::log(&format!("files: cannot open {}", entry.to));
    }
}

#[allow(clippy::too_many_arguments)]
fn open(
    entry: &Entry,
    access: u32,
    share: u32,
    security: *mut c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    // SAFETY: a NUL-terminated path; the other arguments are the game's.
    let h = unsafe {
        win::CreateFileW(
            entry.wide.as_ptr(),
            access,
            share,
            security,
            disposition,
            flags,
            template,
        )
    };
    if h as usize == win::INVALID {
        // SAFETY: plain Win32 calls; the game reads the error of the open.
        let code = unsafe { win::GetLastError() };
        failed(entry);
        unsafe { win::SetLastError(code) };
    }
    h
}

/// The hooks are one function per patched instruction (`SITE`), so that
/// the log names the place of the game that opened a file.
extern "system" fn create_file_a<const SITE: u32>(
    name: *const u8,
    access: u32,
    share: u32,
    security: *mut c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    let entry = std::panic::catch_unwind(|| {
        // SAFETY: the game passes a C string (or null) to CreateFileA.
        unsafe { ansi(name) }
            .and_then(|p| find(&p, "CreateFileA", SITE, reads_only(access, disposition)))
    })
    .ok()
    .flatten();
    if let Some(e) = entry {
        return open(e, access, share, security, disposition, flags, template);
    }
    // SAFETY: what the import slot held: CreateFileA (or another mod's
    // hook with the same arguments).
    let f: CreateFileA = unsafe { std::mem::transmute(slot(IAT_CREATE_FILE_A)) };
    f(name, access, share, security, disposition, flags, template)
}

extern "system" fn create_file_w<const SITE: u32>(
    name: *const u16,
    access: u32,
    share: u32,
    security: *mut c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    let entry = std::panic::catch_unwind(|| {
        // SAFETY: the game passes a wide C string (or null) to CreateFileW.
        unsafe { utf16(name) }
            .and_then(|p| find(&p, "CreateFileW", SITE, reads_only(access, disposition)))
    })
    .ok()
    .flatten();
    if let Some(e) = entry {
        return open(e, access, share, security, disposition, flags, template);
    }
    // SAFETY: as in `create_file_a`.
    let f: CreateFileW = unsafe { std::mem::transmute(slot(IAT_CREATE_FILE_W)) };
    f(name, access, share, security, disposition, flags, template)
}

extern "system" fn get_file_attributes_a<const SITE: u32>(name: *const u8) -> u32 {
    let entry = std::panic::catch_unwind(|| {
        // SAFETY: a C string (or null).
        unsafe { ansi(name) }.and_then(|p| find(&p, "GetFileAttributesA", SITE, true))
    })
    .ok()
    .flatten();
    if let Some(e) = entry {
        // SAFETY: a NUL-terminated path.
        let a = unsafe { win::GetFileAttributesW(e.wide.as_ptr()) };
        if a == u32::MAX {
            failed(e);
        }
        return a;
    }
    // SAFETY: as in `create_file_a`.
    let f: GetFileAttributesA = unsafe { std::mem::transmute(slot(IAT_GET_FILE_ATTRIBUTES_A)) };
    f(name)
}

/// The hook for the patched instruction at `site`: its own instance of
/// the hook (`SITE`), or one without a place for an unknown address.
macro_rules! hook_at {
    ($hook:ident, $ty:ty, $site:expr, [$($at:literal),*]) => {
        match $site {
            $($at => $hook::<$at> as $ty as usize as u32,)*
            _ => $hook::<0> as $ty as usize as u32,
        }
    };
}

fn hook_a(site: u32) -> u32 {
    hook_at!(
        create_file_a,
        CreateFileA,
        site,
        [
            0x00AB_D73D,
            0x00BC_C4DD,
            0x00BC_FC1E,
            0x00BC_FC3D,
            0x00BC_C18C
        ]
    )
}

fn hook_w(site: u32) -> u32 {
    hook_at!(
        create_file_w,
        CreateFileW,
        site,
        [
            0x0069_87FA,
            0x0069_88FD,
            0x0069_9C59,
            0x00AC_0BDF,
            0x00AC_F5C8,
            0x00BC_45A1,
            0x00BC_424F
        ]
    )
}

fn hook_attributes(site: u32) -> u32 {
    hook_at!(
        get_file_attributes_a,
        GetFileAttributesA,
        site,
        [0x00C7_2505]
    )
}

/// Makes the table of `config` and patches the calls of the import slots
/// (after the edition check passed, from `DllMain`: before the game opens
/// `cmd.txt`). Nothing to do when the configuration neither redirects nor
/// logs. Returns the instructions it could not patch.
pub fn install(config: &Config) -> Vec<u32> {
    if !config.hooks_files() {
        return Vec::new();
    }
    let game_dir = game_dir();
    let mut map = HashMap::new();
    for r in &config.redirects {
        let target = if r.relative() {
            format!("{game_dir}\\{}", r.to.replace('/', "\\"))
        } else {
            r.to.clone()
        };
        map.insert(
            r.from.clone(),
            Entry {
                to: r.to.clone(),
                wide: wide(&target),
                opens: AtomicU32::new(0),
                failed: AtomicU32::new(0),
                first: AtomicU32::new(0),
            },
        );
    }
    let _ = TABLE.set(Table {
        game_dir,
        map,
        log_all: config.files_log,
    });
    let mut failed = Vec::new();
    let calls = CREATE_FILE_A_CALLS
        .iter()
        .map(|&at| (at, hook_a(at)))
        .chain(CREATE_FILE_W_CALLS.iter().map(|&at| (at, hook_w(at))))
        .chain(
            GET_FILE_ATTRIBUTES_A_CALLS
                .iter()
                .map(|&at| (at, hook_attributes(at))),
        );
    for (at, hook) in calls {
        // `call [slot]` (six bytes) -> `call hook; nop`.
        let rel = hook.wrapping_sub(at + 5);
        let mut code = [0xE8, 0, 0, 0, 0, 0x90];
        code[1..5].copy_from_slice(&rel.to_le_bytes());
        // SAFETY: an instruction checked by `edition::check`; from DllMain,
        // before the game's threads run.
        if !unsafe { win::patch(at, &code) } {
            failed.push(at);
        }
    }
    let loads = CREATE_FILE_A_LOADS
        .iter()
        .map(|&at| (at, hook_a(at)))
        .chain(CREATE_FILE_W_LOADS.iter().map(|&at| (at, hook_w(at))));
    for (at, hook) in loads {
        // `mov edi, [slot]` (six bytes) -> `mov edi, hook; nop`.
        let mut code = [0xBF, 0, 0, 0, 0, 0x90];
        code[1..5].copy_from_slice(&hook.to_le_bytes());
        // SAFETY: as above.
        if !unsafe { win::patch(at, &code) } {
            failed.push(at);
        }
    }
    HOOKED.store(failed.is_empty(), Ordering::Relaxed);
    failed
}

/// The redirects and their use.
pub fn state() -> Files {
    let Some(table) = TABLE.get() else {
        return Files::default();
    };
    let mut list: Vec<(u32, &String, &Entry)> = table
        .map
        .iter()
        .map(|(k, e)| (e.first.load(Ordering::Relaxed), k, e))
        .collect();
    // Opened ones first, in the order the game opened them; then by name.
    list.sort_by(|a, b| (a.0 == 0, a.0, a.1).cmp(&(b.0 == 0, b.0, b.1)));
    Files {
        hooked: HOOKED.load(Ordering::Relaxed),
        logging: table.log_all,
        total: table.map.len() as u32,
        redirects: list
            .into_iter()
            .take(MAX_LISTED)
            .map(|(_, k, e)| FileUse {
                from: k.clone(),
                to: e.to.clone(),
                opens: e.opens.load(Ordering::Relaxed),
                failed: e.failed.load(Ordering::Relaxed),
            })
            .collect(),
        passed: PASSED.load(Ordering::Relaxed),
    }
}

/// Whether `key` (relative to the game folder) is redirected.
pub fn redirected(key: &str) -> bool {
    TABLE.get().is_some_and(|t| t.map.contains_key(key))
}

/// One line for the log after the installation.
pub fn summary() -> Option<String> {
    let table = TABLE.get()?;
    Some(format!(
        "files: {} redirect(s) from {}{}, {} calls of CreateFileA (0x00D55120), CreateFileW (0x00D55254) and GetFileAttributesA (0x00D5528C) hooked: {}",
        table.map.len(),
        table.game_dir,
        if table.log_all {
            ", logging every open"
        } else {
            ""
        },
        CREATE_FILE_A_CALLS.len()
            + CREATE_FILE_A_LOADS.len()
            + CREATE_FILE_W_CALLS.len()
            + CREATE_FILE_W_LOADS.len()
            + GET_FILE_ATTRIBUTES_A_CALLS.len(),
        if HOOKED.load(Ordering::Relaxed) {
            "on"
        } else {
            "off"
        }
    ))
}
