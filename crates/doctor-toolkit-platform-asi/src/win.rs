//! The few Win32 calls the module needs, reading memory that may not be
//! there, and patching the game's code.

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
    pub fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    pub fn VirtualProtect(address: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    pub fn FlushInstructionCache(process: Handle, address: *const c_void, size: usize) -> i32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetTickCount() -> u32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn DisableThreadLibraryCalls(module: Handle) -> i32;
    pub fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    pub fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
}

/// Whether `len` bytes at `address` can be read.
pub fn readable(address: u32, len: u32) -> bool {
    if address < 0x1_0000 || address.checked_add(len).is_none() {
        return false;
    }
    let mut at = address as usize;
    let end = at + len as usize;
    while at < end {
        let mut info = MemoryBasicInformation::default();
        // SAFETY: fills `info` for any address.
        let n = unsafe {
            VirtualQuery(
                at as *const c_void,
                &mut info,
                std::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        if n == 0
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

/// Reads a value of the game.
///
/// # Safety
/// `address` must be readable memory holding a `T`.
pub unsafe fn read<T: Copy>(address: u32) -> T {
    // SAFETY: the caller's promise.
    unsafe { std::ptr::read_unaligned(address as usize as *const T) }
}

/// Writes a value into data of the game or an object.
///
/// # Safety
/// `address` must be writable memory for a `T`.
pub unsafe fn write<T: Copy>(address: u32, value: T) {
    // SAFETY: the caller's promise.
    unsafe { std::ptr::write_unaligned(address as usize as *mut T, value) }
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
fn peb() -> u32 {
    let peb: u32;
    // SAFETY: reads the PEB pointer of the current thread's TEB.
    unsafe {
        std::arch::asm!("mov {}, fs:[0x30]", out(reg) peb, options(nostack, readonly, preserves_flags))
    };
    peb
}

/// Bases of the modules loaded into the process, in load order
/// (`PEB.Ldr.InMemoryOrderModuleList`; the base of a module is at `+0x10`
/// of its list entry).
pub fn modules() -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: the loader's list of this process, read on the main thread
    // while no module is being loaded; every pointer is checked.
    unsafe {
        let ldr: u32 = read(peb() + 0x0C);
        if !readable(ldr, 0x1C) {
            return out;
        }
        let head = ldr + 0x14;
        let mut entry: u32 = read(head);
        while entry != head && out.len() < 512 && readable(entry, 0x14) {
            let base: u32 = read(entry + 0x10);
            if base != 0 {
                out.push(base);
            }
            entry = read(entry);
        }
    }
    out
}

/// Path of a module by its base.
pub fn module_path(base: u32) -> String {
    let mut buf = [0u16; 1024];
    // SAFETY: fills `buf` with the path of the module.
    let n = unsafe { GetModuleFileNameW(base as usize as Handle, buf.as_mut_ptr(), 1024) };
    String::from_utf16_lossy(&buf[..n as usize])
}
