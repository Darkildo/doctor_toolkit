//! `doctor_render.asi`: the graphics module of The Godfather II (stage 59).
//!
//! The ASI loader (`dinput8.dll`, Ultimate ASI Loader) loads it from the
//! game folder's `scripts/`. On load the module reads `doctor_render.cfg`
//! next to itself ([`doctor_toolkit_live::render::RenderConfig`]), checks
//! the build ([`doctor_toolkit_live::render::check`]) and, only when every
//! place is as expected, patches what the configuration asks for — all of
//! it from `DllMain`, before the game's main loop runs:
//!
//! - [`pace`]: the time of day keeps the rest of every frame; vsync waits
//!   one refresh instead of two; the game's limiter sleeps; a frame limit
//!   of the module's own before `Present`;
//! - [`fov`]: the field of view of the player's camera;
//! - [`post`]: FXAA and sharpening instead of the plain copy of the scene
//!   into the back buffer;
//! - [`memory`]: the test of Large Address Aware (the free addresses below
//!   2 GB reserved) and lines about the address space in the log.
//!
//! It opens no port and writes no game file; its only file is
//! `doctor_render.log` next to it. Nothing unwinds into the game: every
//! entry point catches panics.

#![allow(clippy::missing_safety_doc)]
// The COM methods of Direct3D take as many arguments as they take.
#![allow(clippy::too_many_arguments)]

mod d3d;
mod fov;
mod memory;
mod pace;
mod post;
mod win;

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use doctor_toolkit_live::render::{self, RenderConfig};

/// Version written to the log.
pub const MODULE_VERSION: &str = concat!("doctor_render ", env!("CARGO_PKG_VERSION"));

static MODULE: AtomicU32 = AtomicU32::new(0);
static CONFIG: OnceLock<RenderConfig> = OnceLock::new();
/// What was installed, or why nothing was.
static INSTALLED: OnceLock<Result<Vec<String>, String>> = OnceLock::new();
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// The configuration read in `DllMain`.
pub(crate) fn config() -> &'static RenderConfig {
    CONFIG.get_or_init(RenderConfig::default)
}

/// Folder of the module (`scripts/` of the game).
pub(crate) fn module_dir() -> PathBuf {
    let mut buf = [0u16; 1024];
    let module = MODULE.load(Ordering::Relaxed) as usize as *mut c_void;
    // SAFETY: fills `buf` with the path of this DLL.
    let n = unsafe { win::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) };
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// One line in `doctor_render.log`, with the seconds since the module
/// loaded.
pub(crate) fn log(line: &str) {
    if let Ok(mut file) = LOG.lock() {
        if file.is_none() {
            *file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(module_dir().join(render::LOG_FILE))
                .ok();
        }
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "[{:8.2}] {line}", pace::seconds());
            let _ = f.flush();
        }
    }
}

/// Checks the build and patches what the configuration asks for.
fn install(config: &RenderConfig) -> Result<Vec<String>, String> {
    render::check(&mut |va, buf| {
        // SAFETY: the image of the game is mapped from 0x00400000 to
        // 0x01232000 (its SizeOfImage, checked first by the header) and
        // every checked place lies inside it.
        if !(0x0040_0000..0x0123_2000).contains(&va) {
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(va as usize as *const u8, buf.as_mut_ptr(), buf.len())
        };
        true
    })
    .map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    pace::install(config, &mut done);
    fov::install(config, &mut done);
    post::install(config, &mut done);
    Ok(done)
}

unsafe extern "system" fn start(_: *mut c_void) -> u32 {
    let _ = std::panic::catch_unwind(|| {
        let config = config();
        log(&format!(
            "{MODULE_VERSION} loaded from {}; asked for: {}",
            module_dir().display(),
            config.summary()
        ));
        match INSTALLED.get() {
            Some(Ok(done)) if done.is_empty() => log("edition PC 1.0.789.0; nothing to patch"),
            Some(Ok(done)) => log(&format!("edition PC 1.0.789.0; {}", done.join("; "))),
            Some(Err(why)) => log(&format!("{why}; nothing installed")),
            None => log("not installed"),
        }
        if let Some(line) = memory::reserved_line() {
            log(&line);
        }
        memory::watch(if config.fov_trace {
            10
        } else {
            config.memory_interval()
        });
    });
    0
}

/// Entry point of the DLL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(module: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == win::DLL_PROCESS_ATTACH {
        let _ = std::panic::catch_unwind(|| {
            MODULE.store(module as usize as u32, Ordering::Relaxed);
            // SAFETY: plain Win32 calls allowed under the loader lock.
            unsafe { win::DisableThreadLibraryCalls(module) };
            let config = std::fs::read_to_string(module_dir().join(render::CONFIG_FILE))
                .map(|t| RenderConfig::parse(&t))
                .unwrap_or_default();
            let config = CONFIG.get_or_init(|| config);
            pace::start_clock();
            let installed = install(config);
            // The memory test before the game allocates its heaps.
            if installed.is_ok() && config.low_memory > 0 {
                memory::reserve_low(config.low_memory);
            }
            let _ = INSTALLED.set(installed);
            // SAFETY: a thread of the module; it starts once the loader
            // releases its lock.
            unsafe {
                let h = win::CreateThread(
                    std::ptr::null_mut(),
                    0,
                    start,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                );
                if !h.is_null() {
                    win::CloseHandle(h);
                }
            }
        });
    }
    1
}
