//! The few Win32 calls the module needs, and patching the game's code.

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
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_FREE: u32 = 0x1_0000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const DLL_PROCESS_ATTACH: u32 = 1;

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

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn VirtualQuery(
        address: *const c_void,
        info: *mut MemoryBasicInformation,
        len: usize,
    ) -> usize;
    pub fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    pub fn VirtualProtect(address: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    pub fn FlushInstructionCache(process: Handle, address: *const c_void, size: usize) -> i32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetCurrentThread() -> Handle;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetThreadTimes(
        thread: Handle,
        created: *mut u64,
        exited: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    pub fn GetProcessTimes(
        process: Handle,
        created: *mut u64,
        exited: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
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
    pub fn QueryPerformanceCounter(count: *mut i64) -> i32;
    pub fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    pub fn Sleep(ms: u32);
    pub fn SwitchToThread() -> i32;
    pub fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
}

#[link(name = "winmm")]
unsafe extern "system" {
    pub fn timeBeginPeriod(ms: u32) -> u32;
}

/// `QueryPerformanceCounter`.
pub fn now() -> u64 {
    let mut t = 0i64;
    // SAFETY: fills `t`.
    unsafe { QueryPerformanceCounter(&mut t) };
    t as u64
}

/// `QueryPerformanceFrequency`, at least 1.
pub fn frequency() -> u64 {
    let mut f = 0i64;
    // SAFETY: fills `f`.
    unsafe { QueryPerformanceFrequency(&mut f) };
    (f as u64).max(1)
}

/// Waits until the counter reaches `deadline`: sleeps while more than two
/// milliseconds are left, then yields — no busy core for most of the wait.
pub fn wait_until(deadline: u64, freq: u64) {
    loop {
        let t = now();
        if t >= deadline {
            return;
        }
        let left_ms = (deadline - t) * 1000 / freq;
        if left_ms > 2 {
            // SAFETY: plain Win32 call.
            unsafe { Sleep((left_ms - 2) as u32) };
        } else if left_ms >= 1 {
            // SAFETY: plain Win32 call.
            unsafe { Sleep(0) };
        } else {
            // SAFETY: plain Win32 call.
            unsafe { SwitchToThread() };
        }
    }
}

/// Reads a value of the game's data (an address checked by the edition).
///
/// # Safety
/// `address` must be readable data of the game's image.
pub unsafe fn read<T: Copy>(address: u32) -> T {
    // SAFETY: the caller's promise.
    unsafe { std::ptr::read_volatile(address as usize as *const T) }
}

/// Writes a value of the game's data.
///
/// # Safety
/// `address` must be writable data of the game's image.
pub unsafe fn write<T: Copy>(address: u32, value: T) {
    // SAFETY: the caller's promise.
    unsafe { std::ptr::write_volatile(address as usize as *mut T, value) }
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

/// Points the `call rel32` at `call` to `target`.
///
/// # Safety
/// `call` must be a checked `E8` instruction of the game.
pub unsafe fn redirect_call(call: u32, target: usize) -> bool {
    let rel = (target as u32).wrapping_sub(call + 5);
    // SAFETY: the caller's promise; only the 4 bytes of the target change.
    unsafe { patch(call + 1, &rel.to_le_bytes()) }
}
