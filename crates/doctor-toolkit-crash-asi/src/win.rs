//! The Win32 calls the module needs, reading the process's memory without
//! faulting, and a lock that never blocks a thread of the game for long.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Handle = *mut c_void;

#[repr(C)]
#[derive(Default)]
pub struct MemoryBasicInformation {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// `EXCEPTION_RECORD` of x86.
#[repr(C)]
pub struct ExceptionRecord {
    pub code: u32,
    pub flags: u32,
    pub record: u32,
    pub address: u32,
    pub parameters: u32,
    pub information: [u32; 15],
}

/// `CONTEXT` of x86 (716 bytes) as words; the registers by offset.
#[repr(C)]
pub struct Context {
    pub words: [u32; 179],
}

impl Context {
    pub const GS: usize = 0x8C / 4;
    pub const FS: usize = 0x90 / 4;
    pub const ES: usize = 0x94 / 4;
    pub const DS: usize = 0x98 / 4;
    pub const EDI: usize = 0x9C / 4;
    pub const ESI: usize = 0xA0 / 4;
    pub const EBX: usize = 0xA4 / 4;
    pub const EDX: usize = 0xA8 / 4;
    pub const ECX: usize = 0xAC / 4;
    pub const EAX: usize = 0xB0 / 4;
    pub const EBP: usize = 0xB4 / 4;
    pub const EIP: usize = 0xB8 / 4;
    pub const CS: usize = 0xBC / 4;
    pub const EFLAGS: usize = 0xC0 / 4;
    pub const ESP: usize = 0xC4 / 4;
    pub const SS: usize = 0xC8 / 4;
}

#[repr(C)]
pub struct ExceptionPointers {
    pub record: *mut ExceptionRecord,
    pub context: *mut Context,
}

/// `MINIDUMP_EXCEPTION_INFORMATION` (packed to 4 bytes; 12 bytes on x86).
#[repr(C, packed(4))]
pub struct MinidumpExceptionInformation {
    pub thread: u32,
    pub pointers: *mut ExceptionPointers,
    pub client_pointers: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SystemTime {
    pub year: u16,
    pub month: u16,
    pub day_of_week: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

pub type Filter = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;
/// `MiniDumpWriteDump` of `dbghelp.dll`.
pub type MiniDumpWriteDump = unsafe extern "system" fn(
    process: Handle,
    process_id: u32,
    file: Handle,
    dump_type: u32,
    exception: *const MinidumpExceptionInformation,
    user_streams: *const c_void,
    callback: *const c_void,
) -> i32;

pub const MEM_COMMIT: u32 = 0x1000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_GUARD: u32 = 0x100;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
/// `PAGE_EXECUTE`, `_READ`, `_READWRITE`, `_WRITECOPY`.
pub const PAGE_EXECUTABLE: u32 = 0xF0;
pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const INVALID_HANDLE: usize = usize::MAX;
pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const FILE_SHARE_READ: u32 = 1;
pub const CREATE_ALWAYS: u32 = 2;
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
pub const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
pub const WAIT_OBJECT_0: u32 = 0;
pub const INFINITE: u32 = u32::MAX;
pub const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// `MiniDumpNormal | MiniDumpWithIndirectlyReferencedMemory`: the threads,
/// their stacks, the modules, and the memory the stacks point into.
pub const MINIDUMP_TYPE: u32 = 0x0040;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn VirtualQuery(
        address: *const c_void,
        info: *mut MemoryBasicInformation,
        len: usize,
    ) -> usize;
    pub fn VirtualProtect(address: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    pub fn FlushInstructionCache(process: Handle, address: *const c_void, size: usize) -> i32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn DisableThreadLibraryCalls(module: Handle) -> i32;
    pub fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    pub fn GetCommandLineW() -> *const u16;
    pub fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn CreateEventW(
        attributes: *mut c_void,
        manual_reset: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    pub fn SetEvent(event: Handle) -> i32;
    pub fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
    pub fn Sleep(ms: u32);
    pub fn GetTickCount() -> u32;
    pub fn GetSystemTime(time: *mut SystemTime);
    pub fn GetLocalTime(time: *mut SystemTime);
    pub fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
    pub fn LoadLibraryA(name: *const u8) -> Handle;
    pub fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    pub fn WriteFile(
        file: Handle,
        data: *const u8,
        len: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn FlushFileBuffers(file: Handle) -> i32;
    pub fn GetFileSize(file: Handle, high: *mut u32) -> u32;
    pub fn GetFileAttributesW(name: *const u16) -> u32;
}

/// Whether `[address, address + len)` is committed, readable memory; the
/// region it starts in and that region's protection.
fn region(address: u32) -> Option<(usize, usize, u32)> {
    if address < 0x1_0000 {
        return None;
    }
    let mut info = MemoryBasicInformation::default();
    // SAFETY: VirtualQuery only fills `info`.
    let got = unsafe {
        VirtualQuery(
            address as usize as *const c_void,
            &mut info,
            std::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    if got == 0
        || info.region_size == 0
        || info.state != MEM_COMMIT
        || info.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
        || info.protect == 0
    {
        return None;
    }
    Some((
        info.base_address,
        info.base_address.saturating_add(info.region_size),
        info.protect,
    ))
}

/// What `VirtualQuery` says about the region of an address.
pub fn query(address: u32) -> Option<MemoryBasicInformation> {
    let mut info = MemoryBasicInformation::default();
    // SAFETY: VirtualQuery only fills `info`.
    let got = unsafe {
        VirtualQuery(
            address as usize as *const c_void,
            &mut info,
            std::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    (got != 0).then_some(info)
}

/// Reads the process's memory, checking each range first (one
/// `VirtualQuery` per region, remembered).
#[derive(Default)]
pub struct Mem {
    /// Last region found readable, and its protection.
    ok: (usize, usize, u32),
}

impl Mem {
    fn check(&mut self, address: u32, len: usize) -> bool {
        let mut at = address as usize;
        let Some(end) = at.checked_add(len) else {
            return false;
        };
        if len == 0 {
            return false;
        }
        while at < end {
            if !(at >= self.ok.0 && at < self.ok.1) {
                match region(at as u32) {
                    Some(r) => self.ok = r,
                    None => return false,
                }
            }
            at = self.ok.1;
        }
        true
    }

    pub fn bytes(&mut self, address: u32, out: &mut [u8]) -> bool {
        if !self.check(address, out.len()) {
            return false;
        }
        // SAFETY: the range is committed and readable (checked above).
        unsafe {
            std::ptr::copy_nonoverlapping(
                address as usize as *const u8,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        true
    }

    pub fn u32(&mut self, address: u32) -> Option<u32> {
        let mut b = [0u8; 4];
        self.bytes(address, &mut b).then(|| u32::from_le_bytes(b))
    }

    pub fn u16(&mut self, address: u32) -> Option<u16> {
        let mut b = [0u8; 2];
        self.bytes(address, &mut b).then(|| u16::from_le_bytes(b))
    }

    /// A non-null pointer stored at `address`.
    pub fn ptr(&mut self, address: u32) -> Option<u32> {
        self.u32(address).filter(|&p| p >= 0x1_0000)
    }

    pub fn f32(&mut self, address: u32) -> Option<f32> {
        self.u32(address).map(f32::from_bits)
    }

    /// Whether `address` is readable memory the processor may run.
    pub fn executable(&mut self, address: u32) -> bool {
        self.check(address, 1) && self.ok.2 & PAGE_EXECUTABLE != 0
    }
}

/// Writes `bytes` over code of the game (made writable for the moment).
///
/// # Safety
/// `address` must be code the caller has checked.
pub unsafe fn patch(address: u32, bytes: &[u8]) -> bool {
    let p = address as usize as *mut c_void;
    let mut old = 0u32;
    // SAFETY: the caller checked the address; the protection is restored.
    unsafe {
        if VirtualProtect(p, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let mut ignored = 0u32;
        VirtualProtect(p, bytes.len(), old, &mut ignored);
        FlushInstructionCache(GetCurrentProcess(), p, bytes.len());
    }
    true
}

/// The Process Environment Block of this process.
pub fn peb() -> u32 {
    let peb: u32;
    // SAFETY: reads the PEB pointer of the current thread's TEB.
    unsafe {
        std::arch::asm!("mov {}, fs:[0x30]", out(reg) peb, options(nostack, readonly, preserves_flags))
    };
    peb
}

/// A lock for data the hooks and the crash writer share: a thread that
/// cannot get it after a short spin goes without (the hooks drop a record,
/// the writer of a report reads what it can get later) — a thread that
/// crashed while holding it must not hang the report.
pub struct Spin<T> {
    held: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the value is reached only while `held` is taken.
unsafe impl<T: Send> Sync for Spin<T> {}

impl<T> Spin<T> {
    pub const fn new(value: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Runs `f` with the value; `None` when the lock stayed busy for
    /// `spins` tries.
    pub fn with<R>(&self, spins: u32, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut tries = 0;
        while self
            .held
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            tries += 1;
            if tries > spins {
                return None;
            }
            std::hint::spin_loop();
        }
        // SAFETY: the lock is held.
        let out = f(unsafe { &mut *self.value.get() });
        self.held.store(false, Ordering::Release);
        Some(out)
    }
}
