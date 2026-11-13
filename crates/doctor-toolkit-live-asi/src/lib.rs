//! `doctor_live.asi`: the in-game module of the live link (stage 34).
//!
//! The ASI loader (`dinput8.dll`, Ultimate ASI Loader) loads it from the
//! game folder's `scripts/`. On load the module:
//!
//! 1. reads `doctor_live.cfg` next to itself (port, token, writes);
//! 2. checks the build ([`doctor_toolkit_live::edition`]); on another build
//!    it patches nothing and only answers that the edition is wrong;
//! 3. points the call at `0x0068B04B` of the main loop to its frame hook,
//!    which runs the game's own function afterwards — every read and write
//!    of game data happens there, on the main thread between frames;
//! 4. listens on `127.0.0.1:PORT` in a thread of its own.
//!
//! Nothing here unwinds into the game: every entry point catches panics.
//! The module writes no game file; its log is `doctor_live.log` next to it.

#![allow(clippy::missing_safety_doc)]

mod game;
mod server;
mod win;

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use doctor_toolkit_live::{Request, Response, addr, config::Config, edition};

/// Version reported in Hello.
pub const MODULE_VERSION: &str = concat!("doctor_live ", env!("CARGO_PKG_VERSION"));

static MODULE: AtomicU32 = AtomicU32::new(0);
static FRAME: AtomicU32 = AtomicU32::new(0);
static HOOKED: AtomicBool = AtomicBool::new(false);
static UPDATE_HOOKED: AtomicBool = AtomicBool::new(false);
/// `fix_input` of the configuration ([`game::fix_input`]).
static FIX_INPUT: AtomicBool = AtomicBool::new(false);
/// The player whose input port was checked (once per player object).
static INPUT_CHECKED: AtomicU32 = AtomicU32::new(0);
static EDITION: OnceLock<Result<(), String>> = OnceLock::new();
static SNAPSHOT: Mutex<Option<game::Snapshot>> = Mutex::new(None);
/// The queue of jobs: the network threads send, the main thread receives.
type Queue = (Mutex<Sender<Job>>, Mutex<Receiver<Job>>);
static JOBS: OnceLock<Queue> = OnceLock::new();
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// A request for the main thread.
pub(crate) struct Job {
    pub request: Request,
    pub deadline: Instant,
    pub reply: SyncSender<Response>,
}

fn jobs() -> &'static Queue {
    JOBS.get_or_init(|| {
        let (tx, rx) = channel();
        (Mutex::new(tx), Mutex::new(rx))
    })
}

/// Folder of the module (`scripts/` of the game).
fn module_dir() -> PathBuf {
    let mut buf = [0u16; 1024];
    let module = MODULE.load(Ordering::Relaxed) as usize as *mut c_void;
    // SAFETY: fills `buf` with the path of this DLL.
    let n = unsafe { win::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) };
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// One line in `doctor_live.log`.
pub(crate) fn log(line: &str) {
    if let Ok(mut file) = LOG.lock()
        && let Some(f) = file.as_mut()
    {
        let _ = writeln!(f, "[frame {}] {line}", FRAME.load(Ordering::Relaxed));
        let _ = f.flush();
    }
}

pub(crate) fn edition_state() -> Result<(), String> {
    EDITION
        .get()
        .cloned()
        .unwrap_or_else(|| Err("not checked yet".into()))
}

/// The latest snapshot of the frame hook.
pub(crate) fn snapshot() -> Option<game::Snapshot> {
    SNAPSHOT.lock().ok().and_then(|s| s.clone())
}

pub(crate) fn frames() -> u32 {
    FRAME.load(Ordering::Relaxed)
}

/// Queues a job for the main thread.
pub(crate) fn submit(job: Job) -> bool {
    jobs()
        .0
        .lock()
        .map(|tx| tx.send(job).is_ok())
        .unwrap_or(false)
}

/// Writes waiting for the update of a level frame.
static WRITES: Mutex<Vec<Job>> = Mutex::new(Vec::new());

fn finish(job: Job) {
    let reply = if Instant::now() > job.deadline {
        Response::error(
            doctor_toolkit_live::ErrorCode::Timeout,
            "the game reached the safe point too late; nothing was done",
        )
    } else {
        let request = job.request.clone();
        std::panic::catch_unwind(move || execute(request)).unwrap_or_else(|_| {
            Response::error(doctor_toolkit_live::ErrorCode::Failed, "the module failed")
        })
    };
    let _ = job.reply.try_send(reply);
}

/// Runs the reads that arrived since the last frame; hands the writes to
/// the update of the level ([`update_hook`]), or refuses them outside a
/// level.
fn run_jobs(state: doctor_toolkit_live::LoopState) {
    let Ok(rx) = jobs().1.try_lock() else {
        return;
    };
    // A few per frame: the game runs at 1–60 frames a second.
    for _ in 0..8 {
        let Ok(job) = rx.try_recv() else { break };
        if !job.request.writes() {
            finish(job);
        } else if state != doctor_toolkit_live::LoopState::Game
            || !UPDATE_HOOKED.load(Ordering::Relaxed)
        {
            let _ = job.reply.try_send(Response::error(
                doctor_toolkit_live::ErrorCode::NotReady,
                "not in a level",
            ));
        } else if let Ok(mut w) = WRITES.lock() {
            w.push(job);
        }
    }
}

/// Runs the writes inside a level frame, where the game itself sends its
/// messages and teleports the player: after the frame began
/// (`0x0068BAF0`, the render command buffer `0x01206880` is set) and
/// before the update.
fn run_writes() {
    let jobs: Vec<Job> = match WRITES.try_lock() {
        Ok(mut w) => std::mem::take(&mut *w),
        Err(_) => return,
    };
    for job in jobs {
        finish(job);
    }
}

fn execute(request: Request) -> Response {
    let r = match &request {
        Request::Objects {
            center,
            radius,
            class,
            max,
        } => game::objects(*center, *radius, *class, *max),
        Request::Streams => game::streams(),
        Request::Teleport { position, yaw } => game::teleport(*position, *yaw),
        Request::Send { message, target } => game::send(message.hash(), *target),
        Request::Lua { code } => game::lua(code),
        Request::Peek { address, len } => game::peek(*address, *len),
        _ => Response::error(
            doctor_toolkit_live::ErrorCode::BadRequest,
            "not a main-thread request",
        ),
    };
    if request.writes() {
        log(&format!("{request:?} -> {r:?}"));
    }
    r
}

/// Once per player object, after the controller manager got its port:
/// gives the player that port if he was made without it (`-stream`).
fn check_input() {
    let mut m = win::Mem::default();
    let Some(player) = game::player(&mut m) else {
        return;
    };
    if INPUT_CHECKED.load(Ordering::Relaxed) == player {
        return;
    }
    // Wait until the manager has a device: the update of GameMainLoop gives
    // it one when the level is loaded.
    let has_port = m
        .ptr(addr::CONTROLLER_MANAGER)
        .and_then(|c| m.u8(c + addr::CONTROLLER_PORT))
        .is_some_and(|p| p < 0x10);
    if !has_port {
        return;
    }
    INPUT_CHECKED.store(player, Ordering::Relaxed);
    if let Some(fix) = game::fix_input(&mut m) {
        log(&format!(
            "player {:#010x} read input port {}, given the controller's port {} and moved action maps [{}] (start without the menus)",
            fix.player,
            fix.old,
            fix.port,
            fix.maps.join(", ")
        ));
    }
}

/// The frame hook: the game calls it instead of `0x00439FA0` once per pass
/// of its main loop; it records a snapshot, runs queued jobs and then calls
/// the original function, whose result the loop uses.
extern "C" fn frame_hook() -> u32 {
    let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = std::panic::catch_unwind(|| {
        let snap = game::snapshot(frame);
        let state = doctor_toolkit_live::LoopState::from_u8(snap.state);
        if let Ok(mut s) = SNAPSHOT.try_lock() {
            *s = Some(snap);
        }
        if state == doctor_toolkit_live::LoopState::Game && FIX_INPUT.load(Ordering::Relaxed) {
            check_input();
        }
        run_jobs(state);
    });
    // SAFETY: the function the patched call reached before (checked bytes).
    let original: extern "C" fn() -> u32 =
        unsafe { std::mem::transmute(addr::FRAME_CALLEE as usize) };
    original()
}

/// Stands in the vtable of `GameMainLoop` for its update (`+0x2C`,
/// `0x0067D160`): runs the queued writes, then the game's update, whose
/// status it returns.
extern "thiscall" fn update_hook(this: u32) -> u32 {
    let _ = std::panic::catch_unwind(run_writes);
    // SAFETY: the method the vtable held before (checked bytes).
    let original: extern "thiscall" fn(u32) -> u32 =
        unsafe { std::mem::transmute(addr::GAME_LOOP_UPDATE as usize) };
    original(this)
}

/// Checks the build and installs the frame hook.
fn install() -> Result<(), String> {
    let mut mem = win::Mem::default();
    edition::check(&mut |va, buf| mem.bytes(va, buf)).map_err(|e| e.to_string())?;
    let hook = frame_hook as extern "C" fn() -> u32 as usize as u32;
    let rel = hook.wrapping_sub(addr::FRAME_CALL + 5);
    // SAFETY: the call instruction checked by `edition::check`; only its
    // 4-byte target changes.
    if !unsafe { win::patch(addr::FRAME_CALL + 1, &rel.to_le_bytes()) } {
        return Err("cannot make the main loop writable".into());
    }
    HOOKED.store(true, Ordering::Relaxed);
    let update = update_hook as extern "thiscall" fn(u32) -> u32 as usize as u32;
    // SAFETY: the vtable slot checked by `edition::check`, a 4-byte aligned
    // pointer in the read-only data of the game.
    if unsafe { win::patch(addr::GAME_LOOP_UPDATE_SLOT, &update.to_le_bytes()) } {
        UPDATE_HOOKED.store(true, Ordering::Relaxed);
    }
    Ok(())
}

unsafe extern "system" fn start(_: *mut c_void) -> u32 {
    let _ = std::panic::catch_unwind(|| {
        let dir = module_dir();
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(doctor_toolkit_live::LOG_FILE))
            && let Ok(mut log) = LOG.lock()
        {
            *log = Some(f);
        }
        let config = std::fs::read_to_string(dir.join(doctor_toolkit_live::CONFIG_FILE))
            .map(|t| Config::parse(&t))
            .unwrap_or_default();
        FIX_INPUT.store(config.fix_input, Ordering::Relaxed);
        log(&format!(
            "{MODULE_VERSION} loaded from {}; port {}, writes {}, fix_input {}",
            dir.display(),
            config.port,
            config.writes,
            config.fix_input
        ));
        match edition_state() {
            Ok(()) => log(&format!(
                "edition PC 1.0.789.0, frame hook at 0x0068B04B, update hook at 0x00D57754: {}",
                UPDATE_HOOKED.load(Ordering::Relaxed)
            )),
            Err(e) => log(&format!("{e}; nothing installed, the module only answers")),
        }
        server::run(config);
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
            // The check and the patch need no other DLL: done here, before
            // the game's main loop can run.
            let _ = EDITION.set(install());
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
