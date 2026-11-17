//! `doctor_crash.asi`: the crash module of The Godfather II (stage 99).
//!
//! The ASI loader (`dinput8.dll`, Ultimate ASI Loader) loads it from the
//! game folder's `scripts/`. It needs neither the live module nor the
//! graphics module and gets in the way of neither: a player who only plays
//! mods gets the same report as the editor does.
//!
//! On load the module checks the build
//! ([`doctor_toolkit_live::crash::CHECKS`]); on another build — or when one
//! of its places is taken — it installs nothing and says why in its log.
//! On PC 1.0.789.0 it
//!
//! 1. hooks the message bus and `luaD_throw` ([`hooks`]) to remember the
//!    last messages and the last errors of Lua;
//! 2. installs the process's unhandled-exception filter ([`handler`]).
//!
//! When the game crashes, it writes into `_doctor_toolkit/crashes/` of the
//! game folder a text report ([`report`]: the exception, the place by
//! module and by the names of the knowledge base, registers, return
//! addresses of the stack, the state of the game, the last errors of Lua
//! and messages of the bus, the streams, the enabled mods as the toolkit
//! wrote them, the modules) and a minidump next to it, then lets the crash
//! go on as it would have without the module.
//!
//! It opens no port and writes nowhere else: its log is
//! `_doctor_toolkit/crashes/doctor_crash.log`. Nothing unwinds into the
//! game: every entry point catches panics.

#![allow(clippy::missing_safety_doc)]

mod handler;
mod hooks;
mod report;
mod win;

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use doctor_toolkit_live::crash;
use doctor_toolkit_live::edition;

/// Version written to the log and the reports.
pub const MODULE_VERSION: &str = concat!("doctor_crash ", env!("CARGO_PKG_VERSION"));

static LOADED_TICK: AtomicU32 = AtomicU32::new(0);
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// The game folder: where the executable of the process is.
pub(crate) fn game_dir() -> PathBuf {
    let mut buf = [0u16; 1024];
    // SAFETY: fills `buf` with the path of the process's executable.
    let n = unsafe { win::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), 1024) };
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// `_doctor_toolkit\crashes` of the game folder.
pub(crate) fn crash_dir() -> PathBuf {
    let mut dir = game_dir();
    for part in crash::CRASH_DIR.split('/') {
        dir.push(part);
    }
    dir
}

/// One line in `doctor_crash.log`, with the seconds since the module
/// loaded.
pub(crate) fn log(line: &str) {
    let Ok(mut file) = LOG.lock() else {
        return;
    };
    if file.is_none() {
        let dir = crash_dir();
        let _ = std::fs::create_dir_all(&dir);
        *file = std::fs::File::create(dir.join(crash::LOG_FILE)).ok();
    }
    if let Some(f) = file.as_mut() {
        // SAFETY: a plain Win32 call.
        let ms = unsafe { win::GetTickCount() }.wrapping_sub(LOADED_TICK.load(Ordering::Relaxed));
        let _ = writeln!(f, "[{:5}.{:03}] {line}", ms / 1000, ms % 1000);
        let _ = f.flush();
    }
}

/// Checks the build, then installs the hooks and the filter.
fn install() -> Result<Vec<&'static str>, String> {
    let mut mem = win::Mem::default();
    edition::check_with(&mut |va, buf| mem.bytes(va, buf), crash::CHECKS)
        .map_err(|e| e.to_string())?;
    let failed = hooks::install();
    handler::install(&failed, LOADED_TICK.load(Ordering::Relaxed));
    Ok(failed)
}

/// Entry point of the DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(module: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == win::DLL_PROCESS_ATTACH {
        let _ = std::panic::catch_unwind(|| {
            // SAFETY: plain Win32 calls allowed under the loader lock.
            unsafe {
                win::DisableThreadLibraryCalls(module);
                LOADED_TICK.store(win::GetTickCount(), Ordering::Relaxed);
            }
            // The check and the hooks need no other DLL: done here, before
            // the game's threads run the code they patch.
            let installed = install();
            log(&format!("{MODULE_VERSION} loaded"));
            match installed {
                Ok(failed) if failed.is_empty() => log(&format!(
                    "edition {}; the exception filter is set; hooks: message bus at {:#010x}, luaD_throw at {:#010x}; reports go to {}",
                    edition::NAME,
                    crash::BUS_HOOK,
                    crash::LUA_THROW,
                    crash::CRASH_DIR
                )),
                Ok(failed) => log(&format!(
                    "edition {}; the exception filter is set; could not hook {}",
                    edition::NAME,
                    failed.join(", ")
                )),
                Err(why) => log(&format!("{why}; nothing installed")),
            }
        });
    }
    1
}
