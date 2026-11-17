//! The unhandled-exception filter and the thread that writes the report.
//!
//! The filter runs on the thread that crashed — its stack may be the thing
//! that broke — so it only hands the exception to the module's writer
//! thread and waits for it. The writer was started with the game: its
//! buffer, the folder's path and `dbghelp.dll` are ready before anything
//! crashes. It writes the text report first, then the minidump
//! (`MiniDumpWriteDump`), then the last section of the text.
//!
//! Afterwards the filter passes the exception on as if the module were
//! not there: to the filter that was installed before it (the C runtime of
//! the game sets one), else to the system — the game ends the way it
//! would have (the system's crash window; under Wine, its debugger).
//!
//! The game's runtime replaces the process's filter when it starts; a
//! keeper thread puts the module's filter back once a second and
//! remembers the one it replaced.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use doctor_toolkit_live::crash::{REPORT_PREFIX, TEST_FILE};

use crate::report::{self, Crash, Startup};
use crate::win::{self, ExceptionPointers, Filter, Handle};
use crate::{MODULE_VERSION, hooks, log};

/// The filter the module replaced (0: none).
static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
/// A report is being written or was written: one per run of the game.
static HANDLING: AtomicBool = AtomicBool::new(false);
/// The thread that is inside the previous filter (it may call ours back).
static CHAINING: AtomicU32 = AtomicU32::new(0);
static REQUEST: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);
static POINTERS: AtomicUsize = AtomicUsize::new(0);
static THREAD: AtomicU32 = AtomicU32::new(0);
static WRITER_READY: AtomicBool = AtomicBool::new(false);
static DUMP: AtomicUsize = AtomicUsize::new(0);

/// What the writer prepared when the game started.
struct Ready {
    /// The report's text; its capacity is reserved.
    buffer: String,
    /// `…\_doctor_toolkit\crashes\` in UTF-16, no terminator.
    dir: Vec<u16>,
    context: String,
    cmd_txt: String,
    hooks_failed: String,
    loaded_tick: u32,
}

static READY: Mutex<Option<Ready>> = Mutex::new(None);
static STARTUP: OnceLock<(Vec<u16>, String, u32)> = OnceLock::new();

/// How long the crashed thread waits for the report.
const WAIT_MS: u32 = 60_000;
/// Reserved for a report's text.
const BUFFER: usize = 512 * 1024;

unsafe extern "system" fn filter(info: *mut ExceptionPointers) -> i32 {
    // SAFETY: a plain Win32 call.
    let thread = unsafe { win::GetCurrentThreadId() };
    if CHAINING.load(Ordering::Relaxed) == thread {
        // The previous filter passed the exception back to ours.
        return win::EXCEPTION_CONTINUE_SEARCH;
    }
    if !HANDLING.swap(true, Ordering::SeqCst) {
        POINTERS.store(info as usize, Ordering::SeqCst);
        THREAD.store(thread, Ordering::SeqCst);
        let request = REQUEST.load(Ordering::Relaxed) as Handle;
        let done = DONE.load(Ordering::Relaxed) as Handle;
        if WRITER_READY.load(Ordering::SeqCst) && !request.is_null() && !done.is_null() {
            // SAFETY: events of the module.
            unsafe {
                win::SetEvent(request);
                win::WaitForSingleObject(done, WAIT_MS);
            }
        } else {
            // A crash before the writer started: here, on this thread.
            let _ = std::panic::catch_unwind(|| write_report(info, thread));
        }
    }
    let previous = PREVIOUS.load(Ordering::Relaxed);
    if previous != 0 && previous != filter as Filter as usize {
        CHAINING.store(thread, Ordering::Relaxed);
        // SAFETY: the filter that was installed before ours.
        let previous: Filter = unsafe { std::mem::transmute(previous) };
        let result = unsafe { previous(info) };
        CHAINING.store(0, Ordering::Relaxed);
        return result;
    }
    win::EXCEPTION_CONTINUE_SEARCH
}

/// Puts the module's filter in place; remembers the one it replaced.
/// Returns whether another filter was there.
fn set_filter() -> bool {
    // SAFETY: a plain Win32 call.
    let previous = unsafe { win::SetUnhandledExceptionFilter(Some(filter)) };
    match previous {
        Some(p) if p as usize != filter as Filter as usize => {
            PREVIOUS.store(p as usize, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `dir` + `name` with a terminator, in a buffer on the stack.
fn path(dir: &[u16], name: &str) -> [u16; 600] {
    let mut out = [0u16; 600];
    // The last word stays zero.
    let room = out.len() - 1;
    for (slot, &c) in out[..room]
        .iter_mut()
        .zip(dir.iter().chain(wide_ascii(name).iter()))
    {
        *slot = c;
    }
    out
}

/// An ASCII name in UTF-16, without allocating.
fn wide_ascii(name: &str) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (i, b) in name.bytes().take(63).enumerate() {
        out[i] = u16::from(b);
    }
    out
}

fn create(path: &[u16; 600]) -> Option<Handle> {
    // SAFETY: a NUL-terminated path.
    let file = unsafe {
        win::CreateFileW(
            path.as_ptr(),
            win::GENERIC_WRITE | win::GENERIC_READ,
            win::FILE_SHARE_READ,
            std::ptr::null_mut(),
            win::CREATE_ALWAYS,
            win::FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    (file as usize != win::INVALID_HANDLE && !file.is_null()).then_some(file)
}

fn write_all(file: Handle, data: &[u8]) -> bool {
    let mut at = 0;
    while at < data.len() {
        let mut written = 0u32;
        let chunk = (data.len() - at).min(1 << 20) as u32;
        // SAFETY: `data[at..]` holds `chunk` bytes.
        let ok = unsafe {
            win::WriteFile(
                file,
                data[at..].as_ptr(),
                chunk,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return false;
        }
        at += written as usize;
    }
    true
}

/// `crash-YYYYMMDD-HHMMSS` + extension, as ASCII in a fixed buffer.
fn file_name(t: &win::SystemTime, extension: &str) -> ([u8; 64], usize) {
    use std::io::Write;
    let mut buf = [0u8; 64];
    let mut cursor = std::io::Cursor::new(&mut buf[..]);
    let _ = write!(
        cursor,
        "{REPORT_PREFIX}{:04}{:02}{:02}-{:02}{:02}{:02}{extension}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    );
    let len = cursor.position() as usize;
    (buf, len)
}

/// Writes the report of the exception `info` of `thread`.
fn write_report(info: *mut ExceptionPointers, thread: u32) {
    if info.is_null() {
        return;
    }
    // SAFETY: the pointers the system gave the filter; the crashed thread
    // waits in the filter while they are read.
    let (record, context) = unsafe {
        let info = &*info;
        if info.record.is_null() || info.context.is_null() {
            return;
        }
        (&*info.record, &*info.context)
    };
    let mut utc = win::SystemTime::default();
    let mut local = win::SystemTime::default();
    // SAFETY: plain Win32 calls.
    let tick = unsafe {
        win::GetSystemTime(&mut utc);
        win::GetLocalTime(&mut local);
        win::GetTickCount()
    };
    // What the writer prepared; a crash before that makes do without.
    let mut ready = READY
        .try_lock()
        .ok()
        .and_then(|mut r| r.take())
        .or_else(|| {
            STARTUP.get().map(|(dir, failed, loaded)| Ready {
                buffer: String::with_capacity(64 * 1024),
                dir: dir.clone(),
                context: String::new(),
                cmd_txt: String::new(),
                hooks_failed: failed.clone(),
                loaded_tick: *loaded,
            })
        });
    let Some(ready) = &mut ready else {
        return;
    };
    let mut text = std::mem::take(&mut ready.buffer);
    text.clear();
    let dir = ready.dir.as_slice();
    let crash = Crash {
        record,
        context,
        thread,
        utc,
        local,
        tick,
    };
    let startup = Startup {
        version: MODULE_VERSION,
        loaded_tick: ready.loaded_tick,
        context: &ready.context,
        hooks_failed: &ready.hooks_failed,
        cmd_txt: &ready.cmd_txt,
    };
    report::write(&mut text, &crash, &startup);

    let (name, len) = file_name(&utc, ".txt");
    let name = std::str::from_utf8(&name[..len]).unwrap_or("crash.txt");
    let file = create(&path(dir, name));
    if let Some(file) = file {
        write_all(file, text.as_bytes());
        // SAFETY: the file just made.
        unsafe { win::FlushFileBuffers(file) };
    }

    // The minidump, after the text is on the disk.
    let (dump_name, dump_len) = file_name(&utc, ".dmp");
    let dump_name = std::str::from_utf8(&dump_name[..dump_len]).unwrap_or("crash.dmp");
    let mut tail = String::with_capacity(512);
    tail.push_str("\n[minidump]\n");
    let dump = DUMP.load(Ordering::Relaxed);
    if dump == 0 {
        tail.push_str("# Not written: dbghelp.dll or its MiniDumpWriteDump is not available.\n");
    } else {
        use std::fmt::Write;
        match create(&path(dir, dump_name)) {
            None => tail.push_str("# Not written: the file cannot be made.\n"),
            Some(dump_file) => {
                // SAFETY: `MiniDumpWriteDump` of dbghelp.dll, found when the
                // module started.
                let write: win::MiniDumpWriteDump = unsafe { std::mem::transmute(dump) };
                let exception = win::MinidumpExceptionInformation {
                    thread,
                    pointers: info,
                    client_pointers: 0,
                };
                // SAFETY: the process dumps itself from a thread that did
                // not crash, with the exception of the one that did.
                let (ok, size) = unsafe {
                    let ok = write(
                        win::GetCurrentProcess(),
                        win::GetCurrentProcessId(),
                        dump_file,
                        win::MINIDUMP_TYPE,
                        &exception,
                        std::ptr::null(),
                        std::ptr::null(),
                    );
                    win::FlushFileBuffers(dump_file);
                    let size = win::GetFileSize(dump_file, std::ptr::null_mut());
                    win::CloseHandle(dump_file);
                    (ok, size)
                };
                if ok != 0 {
                    let _ = writeln!(tail, "file: {dump_name}");
                    let _ = writeln!(tail, "size: {size}");
                } else {
                    tail.push_str("# Not written: MiniDumpWriteDump failed.\n");
                    let _ = writeln!(tail, "file: {dump_name}");
                }
            }
        }
    }
    if let Some(file) = file {
        write_all(file, tail.as_bytes());
        // SAFETY: the report's file.
        unsafe {
            win::FlushFileBuffers(file);
            win::CloseHandle(file);
        }
    }
    log(&format!(
        "crash at {:#010x} (code {:#010X}): report {name}",
        record.address, record.code
    ));
}

/// Removes the oldest reports, keeping the newest `keep`.
fn prune(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut reports: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| doctor_toolkit_live::crash::is_report_name(n))
        .collect();
    // The names sort by time.
    reports.sort();
    let extra = reports.len().saturating_sub(keep);
    for name in &reports[..extra] {
        let _ = std::fs::remove_file(dir.join(name));
        let _ = std::fs::remove_file(dir.join(name).with_extension("dmp"));
    }
}

unsafe extern "system" fn writer(_: *mut c_void) -> u32 {
    let _ = std::panic::catch_unwind(|| {
        let dir = crate::crash_dir();
        prune(&dir, doctor_toolkit_live::crash::KEEP_REPORTS);
        // dbghelp.dll now: the loader may be what crashes later.
        // SAFETY: plain Win32 calls with NUL-terminated names.
        let dump = unsafe {
            let dbghelp = win::LoadLibraryA(c"dbghelp.dll".as_ptr().cast());
            if dbghelp.is_null() {
                std::ptr::null_mut()
            } else {
                win::GetProcAddress(dbghelp, c"MiniDumpWriteDump".as_ptr().cast())
            }
        };
        DUMP.store(dump as usize, Ordering::Relaxed);
        log(if dump.is_null() {
            "minidump: dbghelp.dll has no MiniDumpWriteDump here; reports will be text only"
        } else {
            "minidump: MiniDumpWriteDump of dbghelp.dll"
        });
        let context = std::fs::read_to_string(dir.join(doctor_toolkit_live::crash::CONTEXT_FILE))
            .unwrap_or_default();
        log(&format!(
            "context of the launch: {}",
            if context.trim().is_empty() {
                "none (the toolkit did not write context.txt)".to_owned()
            } else {
                format!("{} line(s)", context.lines().count())
            }
        ));
        // The launch options the game reads (`docs/analysis/launch-flags.md`).
        let cmd_txt: String = std::fs::read(crate::game_dir().join("cmd.txt"))
            .map(|bytes| {
                String::from_utf8_lossy(&bytes)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(400)
                    .collect()
            })
            .unwrap_or_default();
        let (wide_dir, failed, loaded) = STARTUP.get().cloned().unwrap_or_default();
        if let Ok(mut ready) = READY.lock() {
            *ready = Some(Ready {
                buffer: String::with_capacity(BUFFER),
                dir: wide_dir,
                context,
                cmd_txt,
                hooks_failed: failed,
                loaded_tick: loaded,
            });
        }
        WRITER_READY.store(true, Ordering::SeqCst);
        let request = REQUEST.load(Ordering::Relaxed) as Handle;
        let done = DONE.load(Ordering::Relaxed) as Handle;
        loop {
            // SAFETY: an event of the module.
            if unsafe { win::WaitForSingleObject(request, win::INFINITE) } != win::WAIT_OBJECT_0 {
                break;
            }
            let info = POINTERS.load(Ordering::SeqCst) as *mut ExceptionPointers;
            let thread = THREAD.load(Ordering::SeqCst);
            let _ = std::panic::catch_unwind(|| write_report(info, thread));
            // SAFETY: an event of the module.
            unsafe { win::SetEvent(done) };
        }
    });
    0
}

/// A crash asked for through [`TEST_FILE`]: its text names the kind.
fn test_crash(dir: &std::path::Path) {
    let file = dir.join(TEST_FILE);
    let Ok(kind) = std::fs::read_to_string(&file) else {
        return;
    };
    let _ = std::fs::remove_file(&file);
    let kind = kind.trim();
    log(&format!("{TEST_FILE}: a crash on purpose, kind `{kind}`"));
    match kind {
        "jump" => hooks::TEST.store(2, Ordering::Relaxed),
        "thread" => {
            // SAFETY: none — a crash on purpose, on this thread.
            unsafe { std::ptr::read_volatile(std::ptr::null::<u32>()) };
        }
        _ => hooks::TEST.store(1, Ordering::Relaxed),
    }
}

unsafe extern "system" fn keeper(_: *mut c_void) -> u32 {
    let _ = std::panic::catch_unwind(|| {
        let dir = crate::crash_dir();
        let trigger = {
            let mut p = wide(&dir.join(TEST_FILE).to_string_lossy());
            p.push(0);
            p
        };
        let mut told = false;
        loop {
            // SAFETY: a plain Win32 call.
            unsafe { win::Sleep(1000) };
            if set_filter() && !told {
                told = true;
                log(&format!(
                    "the game set its own exception filter ({:#010x}); the module's is back, that one runs after it",
                    PREVIOUS.load(Ordering::Relaxed)
                ));
            }
            // SAFETY: a NUL-terminated path.
            if unsafe { win::GetFileAttributesW(trigger.as_ptr()) } != win::INVALID_FILE_ATTRIBUTES
            {
                test_crash(&dir);
            }
        }
    });
    0
}

fn spawn(start: unsafe extern "system" fn(*mut c_void) -> u32) {
    // SAFETY: a thread of the module; under the loader lock it starts once
    // the lock is released.
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
}

/// Installs the filter and starts the module's threads (from `DllMain`).
pub fn install(hooks_failed: &[&str], loaded_tick: u32) {
    let mut dir = wide(&crate::crash_dir().to_string_lossy());
    dir.push(u16::from(b'\\'));
    let _ = STARTUP.set((dir, hooks_failed.join(", "), loaded_tick));
    // SAFETY: plain Win32 calls; auto-reset events without names.
    unsafe {
        REQUEST.store(
            win::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) as usize,
            Ordering::Relaxed,
        );
        DONE.store(
            win::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) as usize,
            Ordering::Relaxed,
        );
    }
    set_filter();
    spawn(writer);
    spawn(keeper);
}
