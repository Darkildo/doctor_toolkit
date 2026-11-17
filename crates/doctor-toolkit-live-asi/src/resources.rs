//! Resources of mods put into the streams the game reads, in memory
//! (stage 100, [`doctor_toolkit_live::resources`]).
//!
//! The configuration names streams of the game folder and their resource
//! patches (`resources=gf2/cuba.str|Z:\…\cuba.str`). When the game opens
//! such a stream ([`opened`], from the hook of `CreateFileW` in
//! [`crate::files`]), the game's own file is opened as always — the game
//! holds a real handle to it — and the module reads its block headers,
//! lays the stream out anew with the resources of the patch and remembers
//! the handle. From then on the game's file device gets its answers for
//! that handle from the composed stream:
//!
//! - the read (`ReadFile` at `0x00ABEC29`, one of the calls the profile
//!   hooks: [`read`] is asked first);
//! - the seek (`SetFilePointerEx` at `0x00ABEC64`, and `SetFilePointer` at
//!   `0x00ABEC78`);
//! - the size (`GetFileSizeEx` at `0x00ABECAE`);
//! - the close (`CloseHandle` at `0x00ABEBE8`) forgets the handle.
//!
//! Unchanged blocks are read from the game's file through that same
//! handle, changed resources from the patch file. A stream that cannot be
//! composed (another file than the patch was made for, a broken patch) is
//! left alone: the game reads its own file, the failure is counted and
//! logged. Nothing is written into the game folder.

use std::collections::HashMap;
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use doctor_toolkit_live::addr::{
    DEVICE_CLOSE_CALL, DEVICE_SEEK_CALL, DEVICE_SIZE_CALL, DEVICE_TELL_CALL, IAT_CLOSE_HANDLE,
    IAT_GET_FILE_SIZE_EX, IAT_SET_FILE_POINTER, IAT_SET_FILE_POINTER_EX, RESOURCE_DATA,
    RESOURCE_FLAGS, RESOURCE_IDS, RESOURCE_NEXT, RESOURCE_REFS, RESOURCE_SIZE, RESOURCE_TABLE,
};
use doctor_toolkit_live::config::Config;
use doctor_toolkit_live::resources::{
    Composed, Loaded, MAX_LISTED, PatchLine, ReadAt, Resources, StreamUse, compose_files,
};

use crate::win::{self, Handle};

/// What the last open of a stream gave.
#[derive(Default, Clone)]
struct Last {
    replaced: u32,
    added: u32,
    original_len: u64,
    virtual_len: u64,
    data_size: u64,
    compose_micros: u32,
    error: String,
}

/// The composed stream of one file of the game, kept while that file has
/// the same length (zones are opened again and again).
struct Made {
    original_len: u64,
    composed: Arc<Composed>,
    patch: Arc<File>,
    base: Option<Arc<File>>,
}

struct Stream {
    from: String,
    to: String,
    /// The file to compose from instead of the game's own stream.
    base: Option<String>,
    opens: AtomicU32,
    failed: AtomicU32,
    /// Order of the first open (0: not opened yet).
    first: AtomicU32,
    bytes_read: AtomicU64,
    last: Mutex<Last>,
    made: Mutex<Option<Made>>,
    dumped: AtomicBool,
}

/// A handle of the game the module answers for.
struct Open {
    stream: &'static Stream,
    composed: Arc<Composed>,
    patch: Arc<File>,
    /// The file the unchanged blocks come from, when not the game's handle.
    base: Option<Arc<File>>,
    /// Where the game stands in the virtual stream.
    pos: Mutex<u64>,
}

impl Open {
    /// Reads the virtual stream at `at` for the game's `handle`.
    fn read(
        &self,
        handle: Handle,
        at: u64,
        buf: &mut [u8],
    ) -> Result<usize, doctor_toolkit_live::resources::Error> {
        let patch = &mut PatchFile(&self.patch);
        match &self.base {
            Some(base) => self.composed.read(at, buf, &mut PatchFile(base), patch),
            None => self.composed.read(at, buf, &mut Original(handle), patch),
        }
    }
}

struct Table {
    streams: HashMap<String, Stream>,
    dump: bool,
}

static TABLE: OnceLock<Table> = OnceLock::new();
static HOOKED: AtomicBool = AtomicBool::new(false);
static FIRSTS: AtomicU32 = AtomicU32::new(0);
/// Handles answered from a composed stream.
static OPEN: RwLock<Option<HashMap<usize, Arc<Open>>>> = RwLock::new(None);
/// How many there are: the hooks pass every other handle on without a lock.
static OPEN_COUNT: AtomicU32 = AtomicU32::new(0);

const FILE_BEGIN: u32 = 0;
const FILE_CURRENT: u32 = 1;
const FILE_END: u32 = 2;
const ERROR_READ_FAULT: u32 = 30;
const ERROR_NEGATIVE_SEEK: u32 = 131;

/// The game's own file, read through the handle the game holds (the
/// module's own imports: not the hooked calls).
struct Original(Handle);

impl ReadAt for Original {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        let mut done = 0usize;
        // SAFETY: plain Win32 calls on a file handle opened for reading;
        // the buffer is ours.
        unsafe {
            if win::SetFilePointerEx(self.0, offset as i64, std::ptr::null_mut(), FILE_BEGIN) == 0 {
                return false;
            }
            while done < buf.len() {
                let mut n = 0u32;
                let want = (buf.len() - done).min(1 << 30) as u32;
                if win::ReadFile(
                    self.0,
                    buf.as_mut_ptr().add(done),
                    want,
                    &mut n,
                    std::ptr::null_mut(),
                ) == 0
                    || n == 0
                {
                    return false;
                }
                done += n as usize;
            }
        }
        true
    }

    fn len(&mut self) -> Option<u64> {
        let mut size = 0i64;
        // SAFETY: fills `size`.
        (unsafe { win::GetFileSizeEx(self.0, &mut size) } != 0).then_some(size as u64)
    }
}

/// The patch file, or a file the module opened to compose from.
struct PatchFile<'a>(&'a File);

impl ReadAt for PatchFile<'_> {
    fn read_at(&mut self, mut offset: u64, buf: &mut [u8]) -> bool {
        let mut done = 0usize;
        while done < buf.len() {
            match self.0.seek_read(&mut buf[done..], offset) {
                Ok(0) | Err(_) => return false,
                Ok(n) => {
                    done += n;
                    offset += n as u64;
                }
            }
        }
        true
    }

    fn len(&mut self) -> Option<u64> {
        self.0.metadata().ok().map(|m| m.len())
    }
}

fn open_handles<R>(f: impl FnOnce(&mut HashMap<usize, Arc<Open>>) -> R) -> Option<R> {
    let mut guard = OPEN.write().ok()?;
    Some(f(guard.get_or_insert_with(HashMap::new)))
}

fn lookup(handle: Handle) -> Option<Arc<Open>> {
    if OPEN_COUNT.load(Ordering::Relaxed) == 0 {
        return None;
    }
    OPEN.read().ok()?.as_ref()?.get(&(handle as usize)).cloned()
}

/// Forgets a handle: the game closed it, or the system gave its number to
/// another file.
pub fn forget(handle: Handle) {
    if OPEN_COUNT.load(Ordering::Relaxed) == 0 {
        return;
    }
    open_handles(|map| {
        if map.remove(&(handle as usize)).is_some() {
            OPEN_COUNT.store(map.len() as u32, Ordering::Relaxed);
        }
    });
}

/// Whether the configuration has a patch for the file `key` of the game
/// folder.
pub fn patched(key: &str) -> bool {
    TABLE.get().is_some_and(|t| t.streams.contains_key(key))
}

/// Composes the stream `stream` for the game's handle.
fn make(stream: &'static Stream, handle: Handle, dump: bool) -> Result<Arc<Open>, String> {
    // The file the patch was made for: the game's own stream through the
    // handle the game holds, or another file the configuration names.
    let base = match &stream.base {
        Some(path) => Some(Arc::new(
            File::open(path).map_err(|e| format!("{path}: {e}"))?,
        )),
        None => None,
    };
    let mut game_file = Original(handle);
    let mut base_file = base.as_deref().map(PatchFile);
    let original: &mut dyn ReadAt = match &mut base_file {
        Some(file) => file,
        None => &mut game_file,
    };
    let len = original
        .len()
        .ok_or_else(|| "cannot ask the size of the stream".to_owned())?;
    let mut made = stream.made.lock().map_err(|_| "poisoned".to_owned())?;
    if let Some(m) = made.as_ref().filter(|m| m.original_len == len) {
        return Ok(Arc::new(Open {
            stream,
            composed: m.composed.clone(),
            patch: m.patch.clone(),
            base: m.base.clone(),
            pos: Mutex::new(0),
        }));
    }
    let started = Instant::now();
    let patch = File::open(&stream.to).map_err(|e| format!("{}: {e}", stream.to))?;
    let (layout, _, composed) =
        compose_files(original, &mut PatchFile(&patch)).map_err(|e| e.to_string())?;
    let micros = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
    if let Ok(mut last) = stream.last.lock() {
        *last = Last {
            replaced: composed.replaced,
            added: composed.added,
            original_len: len,
            virtual_len: composed.len,
            data_size: composed.data_size,
            compose_micros: micros,
            error: String::new(),
        };
    }
    crate::log(&format!(
        "resources: {}{} composed in {micros} us: {} resources of the game ({} blocks, {} reads), {} replaced, {} added{}; {} -> {} bytes in {} pieces, data size {}",
        stream.from,
        match &stream.base {
            Some(base) => format!(" (from {base})"),
            None => String::new(),
        },
        layout.resources.len(),
        layout.blocks.len(),
        layout.reads,
        composed.replaced,
        composed.added,
        if composed.misplaced == 0 {
            String::new()
        } else {
            format!(
                " ({} at the end: their place is not in the stream)",
                composed.misplaced
            )
        },
        len,
        composed.len,
        composed.segments.len(),
        composed.data_size
    ));
    if dump && !stream.dumped.swap(true, Ordering::Relaxed) {
        let to = format!("{}.composed", stream.to);
        let result = composed
            .to_vec(original, &mut PatchFile(&patch))
            .map_err(|e| e.to_string())
            .and_then(|all| std::fs::write(&to, all).map_err(|e| e.to_string()));
        match result {
            Ok(()) => crate::log(&format!("resources: {} written to {to}", stream.from)),
            Err(e) => crate::log(&format!("resources: cannot write {to}: {e}")),
        }
    }
    let composed = Arc::new(composed);
    let patch = Arc::new(patch);
    *made = Some(Made {
        original_len: len,
        composed: composed.clone(),
        patch: patch.clone(),
        base: base.clone(),
    });
    Ok(Arc::new(Open {
        stream,
        composed,
        patch,
        base,
        pos: Mutex::new(0),
    }))
}

/// The game opened the file `key` of its folder for reading and got
/// `handle` (its own file). When the configuration has a patch for it, the
/// handle is answered from the composed stream from now on.
pub fn opened(key: &str, handle: Handle) {
    let Some(table) = TABLE.get() else {
        return;
    };
    let Some(stream) = table.streams.get(key) else {
        return;
    };
    if stream.opens.fetch_add(1, Ordering::Relaxed) == 0 {
        stream.first.store(
            FIRSTS.fetch_add(1, Ordering::Relaxed) + 1,
            Ordering::Relaxed,
        );
    }
    let result = make(stream, handle, table.dump);
    // The scan moved the file: the game expects it at its start, and reads
    // it itself when the stream was not composed.
    // SAFETY: a plain Win32 call on the handle just opened.
    unsafe { win::SetFilePointerEx(handle, 0, std::ptr::null_mut(), FILE_BEGIN) };
    match result {
        Ok(open) => {
            open_handles(|map| {
                map.insert(handle as usize, open);
                OPEN_COUNT.store(map.len() as u32, Ordering::Relaxed);
            });
        }
        Err(error) => {
            if stream.failed.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::log(&format!(
                    "resources: {} is not composed, the game reads its own file: {error}",
                    stream.from
                ));
            }
            if let Ok(mut last) = stream.last.lock() {
                last.error = error;
            }
        }
    }
}

/// The read of the game: `Some(result of ReadFile)` when `handle` is
/// answered from a composed stream.
///
/// # Safety
/// The arguments are those of the game's `ReadFile` call.
pub unsafe fn read(
    handle: Handle,
    buffer: *mut u8,
    len: u32,
    read: *mut u32,
    overlapped: *mut u8,
) -> Option<i32> {
    let open = lookup(handle)?;
    let result = std::panic::catch_unwind(|| {
        let mut pos = open.pos.lock().ok()?;
        // The game passes no OVERLAPPED; with one, its offset is the place.
        let at = if overlapped.is_null() {
            *pos
        } else {
            // SAFETY: an OVERLAPPED of the caller: Offset and OffsetHigh at
            // +8 and +12.
            unsafe {
                u64::from(std::ptr::read_unaligned(overlapped.add(8) as *const u32))
                    | u64::from(std::ptr::read_unaligned(overlapped.add(12) as *const u32)) << 32
            }
        };
        // SAFETY: the game's buffer of `len` bytes.
        let buf = unsafe { std::slice::from_raw_parts_mut(buffer, len as usize) };
        let n = open.read(handle, at, buf);
        match n {
            Ok(n) => {
                *pos = at + n as u64;
                open.stream
                    .bytes_read
                    .fetch_add(n as u64, Ordering::Relaxed);
                Some(n as u32)
            }
            Err(error) => {
                if let Ok(mut last) = open.stream.last.lock()
                    && last.error.is_empty()
                {
                    crate::log(&format!("resources: {}: {error}", open.stream.from));
                    last.error = error.to_string();
                }
                None
            }
        }
    })
    .ok()
    .flatten();
    // SAFETY: the out-parameter of the game's call (never null there).
    unsafe {
        if !read.is_null() {
            std::ptr::write_unaligned(read, result.unwrap_or(0));
        }
        Some(match result {
            Some(_) => {
                win::SetLastError(0);
                1
            }
            None => {
                win::SetLastError(ERROR_READ_FAULT);
                0
            }
        })
    }
}

/// Moves the place of a composed stream; `None` for a negative place.
fn seek(open: &Open, distance: i64, method: u32) -> Option<u64> {
    let mut pos = open.pos.lock().ok()?;
    let base = match method {
        FILE_BEGIN => 0,
        FILE_CURRENT => *pos as i64,
        FILE_END => open.composed.len as i64,
        _ => return None,
    };
    let to = u64::try_from(base.checked_add(distance)?).ok()?;
    *pos = to;
    Some(to)
}

fn slot<F: Copy>(address: u32) -> F {
    // SAFETY: an import slot checked by `edition::check`, a 4-byte aligned
    // pointer the loader filled; it holds a function of the type asked.
    unsafe {
        let f = std::ptr::read_volatile(address as usize as *const u32) as usize;
        std::mem::transmute_copy::<usize, F>(&f)
    }
}

type SetFilePointerEx = extern "system" fn(Handle, u32, i32, *mut i64, u32) -> i32;
type SetFilePointer = extern "system" fn(Handle, i32, *mut i32, u32) -> u32;
type GetFileSizeEx = extern "system" fn(Handle, *mut i64) -> i32;
type CloseHandle = extern "system" fn(Handle) -> i32;

/// `SetFilePointerEx(handle, distance, &new, method)`; the 64-bit distance
/// is passed in two words.
extern "system" fn set_file_pointer_ex(
    handle: Handle,
    low: u32,
    high: i32,
    new: *mut i64,
    method: u32,
) -> i32 {
    if let Some(open) = lookup(handle) {
        let distance = i64::from(high) << 32 | i64::from(low);
        return match seek(&open, distance, method) {
            Some(to) => {
                if !new.is_null() {
                    // SAFETY: the out-parameter of the game's call.
                    unsafe { std::ptr::write_unaligned(new, to as i64) };
                }
                1
            }
            None => {
                // SAFETY: a plain Win32 call.
                unsafe { win::SetLastError(ERROR_NEGATIVE_SEEK) };
                0
            }
        };
    }
    let f: SetFilePointerEx = slot(IAT_SET_FILE_POINTER_EX);
    f(handle, low, high, new, method)
}

extern "system" fn set_file_pointer(handle: Handle, low: i32, high: *mut i32, method: u32) -> u32 {
    if let Some(open) = lookup(handle) {
        let distance = if high.is_null() {
            i64::from(low)
        } else {
            // SAFETY: the in/out high part of the game's call.
            i64::from(unsafe { std::ptr::read_unaligned(high) }) << 32 | i64::from(low as u32)
        };
        return match seek(&open, distance, method) {
            Some(to) => {
                // SAFETY: as above; the error tells a low part of
                // 0xFFFFFFFF from a failure.
                unsafe {
                    if !high.is_null() {
                        std::ptr::write_unaligned(high, (to >> 32) as i32);
                    }
                    win::SetLastError(0);
                }
                to as u32
            }
            None => {
                // SAFETY: a plain Win32 call.
                unsafe { win::SetLastError(ERROR_NEGATIVE_SEEK) };
                u32::MAX
            }
        };
    }
    let f: SetFilePointer = slot(IAT_SET_FILE_POINTER);
    f(handle, low, high, method)
}

extern "system" fn get_file_size_ex(handle: Handle, size: *mut i64) -> i32 {
    if let Some(open) = lookup(handle) {
        if !size.is_null() {
            // SAFETY: the out-parameter of the game's call.
            unsafe { std::ptr::write_unaligned(size, open.composed.len as i64) };
        }
        return 1;
    }
    let f: GetFileSizeEx = slot(IAT_GET_FILE_SIZE_EX);
    f(handle, size)
}

extern "system" fn close_handle(handle: Handle) -> i32 {
    let _ = std::panic::catch_unwind(|| forget(handle));
    let f: CloseHandle = slot(IAT_CLOSE_HANDLE);
    f(handle)
}

/// Makes the table of `config` and patches the calls of the file device
/// (after the edition check, from `DllMain`). Nothing to do without
/// `resources=` lines. Returns the instructions it could not patch.
pub fn install(config: &Config, game_dir: &str) -> Vec<u32> {
    if config.resources.is_empty() {
        return Vec::new();
    }
    let mut streams = HashMap::new();
    for r in &config.resources {
        streams.insert(
            r.from.clone(),
            Stream {
                from: r.from.clone(),
                to: PatchLine::resolve(&r.to, game_dir),
                base: r.base.as_deref().map(|b| PatchLine::resolve(b, game_dir)),
                opens: AtomicU32::new(0),
                failed: AtomicU32::new(0),
                first: AtomicU32::new(0),
                bytes_read: AtomicU64::new(0),
                last: Mutex::new(Last::default()),
                made: Mutex::new(None),
                dumped: AtomicBool::new(false),
            },
        );
    }
    let _ = TABLE.set(Table {
        streams,
        dump: config.resources_dump,
    });
    let hooks = [
        (
            DEVICE_SEEK_CALL,
            set_file_pointer_ex as SetFilePointerEx as usize as u32,
        ),
        (
            DEVICE_TELL_CALL,
            set_file_pointer as SetFilePointer as usize as u32,
        ),
        (
            DEVICE_SIZE_CALL,
            get_file_size_ex as GetFileSizeEx as usize as u32,
        ),
        (
            DEVICE_CLOSE_CALL,
            close_handle as CloseHandle as usize as u32,
        ),
    ];
    let mut failed = Vec::new();
    for (at, hook) in hooks {
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
    HOOKED.store(failed.is_empty(), Ordering::Relaxed);
    failed
}

/// The patched streams and their use.
pub fn state() -> Resources {
    let Some(table) = TABLE.get() else {
        return Resources::default();
    };
    let mut list: Vec<(u32, &Stream)> = table
        .streams
        .values()
        .map(|s| (s.first.load(Ordering::Relaxed), s))
        .collect();
    // Opened ones first, in the order the game opened them; then by name.
    list.sort_by(|a, b| (a.0 == 0, a.0, &a.1.from).cmp(&(b.0 == 0, b.0, &b.1.from)));
    Resources {
        hooked: HOOKED.load(Ordering::Relaxed),
        total: table.streams.len() as u32,
        open: OPEN_COUNT.load(Ordering::Relaxed),
        streams: list
            .into_iter()
            .take(MAX_LISTED)
            .map(|(_, s)| {
                let last = s.last.lock().map(|l| l.clone()).unwrap_or_default();
                StreamUse {
                    from: s.from.clone(),
                    to: s.to.clone(),
                    base: s.base.clone().unwrap_or_default(),
                    opens: s.opens.load(Ordering::Relaxed),
                    failed: s.failed.load(Ordering::Relaxed),
                    replaced: last.replaced,
                    added: last.added,
                    original_len: last.original_len,
                    virtual_len: last.virtual_len,
                    data_size: last.data_size,
                    compose_micros: last.compose_micros,
                    bytes_read: s.bytes_read.load(Ordering::Relaxed),
                    error: last.error,
                }
            })
            .collect(),
    }
}

/// The record of the loaded resource with these ids in the game's table
/// (`*0x012053AC`): the bucket of its hash is read as the game's own
/// lookup (`0x00421170`) reads it; the game's function is not called. The
/// loader may change the table meanwhile; every read is checked, a chain
/// is followed for at most 65 536 records.
pub fn loaded(ids: [u32; 4]) -> Option<Loaded> {
    let mut m = win::Mem::default();
    let table = m.ptr(RESOURCE_TABLE)?;
    let buckets = m.ptr(table)?;
    let count = m.u32(table + 4)?;
    if count == 0 {
        return None;
    }
    let bucket = doctor_toolkit_live::resources::table_hash(ids) % count;
    let mut steps = 0u32;
    {
        let mut record = m.u32(buckets + bucket * 4)?;
        while record >= 0x1_0000 && steps < 65_536 {
            steps += 1;
            let mut found = [0u8; 16];
            if !m.bytes(record + RESOURCE_IDS, &mut found) {
                break;
            }
            let same = ids
                .iter()
                .enumerate()
                .all(|(k, id)| found[k * 4..k * 4 + 4] == id.to_le_bytes());
            if same {
                return Some(Loaded {
                    record,
                    data: m.u32(record + RESOURCE_DATA)?,
                    size: m.u32(record + RESOURCE_SIZE)?,
                    refs: m.u32(record + RESOURCE_REFS)?,
                    flags: (m.u32(record + RESOURCE_FLAGS - 2)? >> 16) as u16,
                });
            }
            record = m.u32(record + RESOURCE_NEXT)?;
        }
    }
    None
}

/// One line for the log after the installation.
pub fn summary() -> Option<String> {
    let table = TABLE.get()?;
    Some(format!(
        "resources: {} stream(s) composed from a resource patch{}; the file device's seek (0x00ABEC64, 0x00ABEC78), size (0x00ABECAE) and close (0x00ABEBE8) hooked: {}; its read (0x00ABEC29) through the hook of the profile",
        table.streams.len(),
        if table.dump {
            ", each written next to its patch"
        } else {
            ""
        },
        if HOOKED.load(Ordering::Relaxed) {
            "on"
        } else {
            "off"
        }
    ))
}
