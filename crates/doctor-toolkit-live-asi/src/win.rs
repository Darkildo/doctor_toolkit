//! The few Win32 calls the module needs, and reading the game's memory
//! without faulting.

use std::ffi::c_void;

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

pub const MEM_COMMIT: u32 = 0x1000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_GUARD: u32 = 0x100;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const DLL_PROCESS_ATTACH: u32 = 1;

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
    pub fn DisableThreadLibraryCalls(module: Handle) -> i32;
    pub fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    pub fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    pub fn GetFileAttributesW(name: *const u16) -> u32;
    pub fn GetCurrentDirectoryW(size: u32, buffer: *mut u16) -> u32;
    pub fn GetLastError() -> u32;
    pub fn SetLastError(code: u32);
    pub fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        text: *const u8,
        len: i32,
        out: *mut u16,
        out_len: i32,
    ) -> i32;
    // Stage 58: the profile.
    pub fn QueryPerformanceCounter(count: *mut i64) -> i32;
    pub fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    pub fn OpenThread(access: u32, inherit: i32, id: u32) -> Handle;
    pub fn GetThreadTimes(
        thread: Handle,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    pub fn GetProcessTimes(
        process: Handle,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    pub fn SuspendThread(thread: Handle) -> u32;
    pub fn ResumeThread(thread: Handle) -> u32;
    pub fn GetThreadContext(thread: Handle, context: *mut Context) -> i32;
    pub fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    pub fn Sleep(ms: u32);
}

pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_FREE: u32 = 0x1_0000;
/// `THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION`.
pub const THREAD_SAMPLE: u32 = 0x0002 | 0x0008 | 0x0040;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;
/// `CONTEXT_i386 | CONTEXT_CONTROL`: `Ebp`, `Eip`, `SegCs`, `EFlags`,
/// `Esp`, `SegSs`.
pub const CONTEXT_CONTROL: u32 = 0x0001_0001;

/// `CONTEXT` of x86 (716 bytes): `ContextFlags` at 0, `Eip` at `0xB8`.
#[repr(C, align(16))]
pub struct Context {
    pub words: [u32; 179],
}

impl Context {
    pub const FLAGS: usize = 0;
    pub const EIP: usize = 0xB8 / 4;
    pub const ESP: usize = 0xC4 / 4;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `ThreadBasicInformation` (class 0, 28 bytes): `TebBaseAddress` at 4.
    pub fn NtQueryInformationThread(
        thread: Handle,
        class: u32,
        info: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// `MEMORYSTATUSEX`.
#[repr(C)]
#[derive(Default)]
pub struct MemoryStatusEx {
    pub length: u32,
    pub memory_load: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page_file: u64,
    pub avail_page_file: u64,
    pub total_virtual: u64,
    pub avail_virtual: u64,
    pub avail_extended_virtual: u64,
}

/// `INVALID_HANDLE_VALUE` and `INVALID_FILE_ATTRIBUTES`.
pub const INVALID: usize = usize::MAX;

/// Whether `[address, address + len)` is committed, readable memory.
pub fn readable(address: u32, len: usize) -> bool {
    if address < 0x1_0000 || len == 0 {
        return false;
    }
    let mut at = address as usize;
    let Some(end) = at.checked_add(len) else {
        return false;
    };
    while at < end {
        let mut info = MemoryBasicInformation::default();
        // SAFETY: VirtualQuery only fills `info`.
        let got = unsafe {
            VirtualQuery(
                at as *const c_void,
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
            return false;
        }
        at = info.base_address + info.region_size;
    }
    true
}

/// Readable, committed regions of the address space, sorted (one walk of
/// `VirtualQuery`; Wine answers each query slowly, so the jobs that read a
/// lot check against this map instead of asking per read).
fn readable_regions() -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut at = 0x1_0000usize;
    while at < 0xFFFF_0000 {
        let mut info = MemoryBasicInformation::default();
        // SAFETY: VirtualQuery only fills `info`.
        let got = unsafe {
            VirtualQuery(
                at as *const c_void,
                &mut info,
                std::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        if got == 0 || info.region_size == 0 {
            break;
        }
        let end = info.base_address.saturating_add(info.region_size);
        if info.state == MEM_COMMIT
            && info.protect != 0
            && info.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0
        {
            match out.last_mut() {
                Some(last) if last.1 == info.base_address => last.1 = end,
                _ => out.push((info.base_address, end)),
            }
        }
        if end <= at {
            break;
        }
        at = end;
    }
    out
}

/// Reads game memory, checking each range first. The game is paused at the
/// frame hook while the module reads, so what it checked stays mapped.
#[derive(Default)]
pub struct Mem {
    /// Last range found readable.
    ok: (usize, usize),
    /// All readable ranges, when the reader was made by [`Mem::mapped`].
    map: Option<Vec<(usize, usize)>>,
}

impl Mem {
    /// A reader for many reads: maps the address space once.
    pub fn mapped() -> Self {
        Self {
            ok: (0, 0),
            map: Some(readable_regions()),
        }
    }

    fn check(&mut self, address: u32, len: usize) -> bool {
        let a = address as usize;
        let Some(end) = a.checked_add(len) else {
            return false;
        };
        if a >= self.ok.0 && end <= self.ok.1 {
            return true;
        }
        if let Some(map) = &self.map {
            let i = map.partition_point(|r| r.1 <= a);
            return match map.get(i) {
                Some(&(start, stop)) if start <= a && end <= stop => {
                    self.ok = (start, stop);
                    true
                }
                _ => false,
            };
        }
        if !readable(address, len) {
            return false;
        }
        let mut info = MemoryBasicInformation::default();
        // SAFETY: as above.
        unsafe {
            VirtualQuery(
                a as *const c_void,
                &mut info,
                std::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        self.ok = (info.base_address, info.base_address + info.region_size);
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

    pub fn u8(&mut self, address: u32) -> Option<u8> {
        let mut b = [0u8; 1];
        self.bytes(address, &mut b).then_some(b[0])
    }

    /// A non-null pointer stored at `address`.
    pub fn ptr(&mut self, address: u32) -> Option<u32> {
        self.u32(address).filter(|&p| p >= 0x1_0000)
    }

    pub fn f32s<const N: usize>(&mut self, address: u32) -> Option<[f32; N]> {
        let mut out = [0f32; N];
        let mut b = vec![0u8; N * 4];
        if !self.bytes(address, &mut b) {
            return None;
        }
        for (i, v) in out.iter_mut().enumerate() {
            *v = f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        }
        Some(out)
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
        // A 4-byte aligned store is atomic: a thread running past the
        // patched call sees the old or the new target, never half of each.
        if bytes.len() == 4 && address.is_multiple_of(4) {
            std::ptr::write_volatile(
                p as *mut u32,
                u32::from_le_bytes(bytes.try_into().unwrap_or_default()),
            );
        } else {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        }
        let mut ignored = 0u32;
        VirtualProtect(p, bytes.len(), old, &mut ignored);
        FlushInstructionCache(GetCurrentProcess(), p, bytes.len());
    }
    true
}
