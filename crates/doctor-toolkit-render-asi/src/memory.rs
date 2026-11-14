//! The address space: the test of Large Address Aware and lines about
//! memory in the log (`docs/analysis/performance.md` §5).
//!
//! With the flag `LARGE_ADDRESS_AWARE` (a copy of the executable, made by
//! "Play") the game may get addresses above 2 GB, but Wine and Windows hand
//! out the low ones first: the game's own memory stays below and pointers
//! above `0x80000000` are never tried. With `low_memory=N` the module
//! reserves every free range below 2 GB except N megabytes, at load time
//! — before the game makes its heaps — so that what the game allocates
//! later lands above. A crash or wrong behaviour then points at signed
//! comparisons of pointers in the game.

use std::ffi::c_void;
use std::sync::Mutex;

use crate::win;

const TWO_GB: usize = 0x8000_0000;
const GRANULARITY: usize = 0x1_0000;
const MB: usize = 1 << 20;

/// What [`reserve_low`] did: reserved bytes, ranges, bytes left free.
static RESERVED: Mutex<Option<(usize, usize, usize)>> = Mutex::new(None);

/// Free ranges between `from` and `to`, aligned to the allocation
/// granularity.
fn free_ranges(from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let mut info = win::MemoryBasicInformation::default();
        // SAFETY: VirtualQuery only fills `info`.
        let got = unsafe {
            win::VirtualQuery(
                at as *const c_void,
                &mut info,
                std::mem::size_of::<win::MemoryBasicInformation>(),
            )
        };
        if got == 0 || info.region_size == 0 {
            break;
        }
        let end = info.base_address.saturating_add(info.region_size).min(to);
        if info.state == win::MEM_FREE {
            let start = info.base_address.max(at).next_multiple_of(GRANULARITY);
            let stop = end & !(GRANULARITY - 1);
            if stop > start {
                out.push((start, stop));
            }
        }
        if end <= at {
            break;
        }
        at = end;
    }
    out
}

/// Reserves the free addresses below 2 GB except `leave_mb` megabytes at
/// the start of the largest free range.
pub fn reserve_low(leave_mb: u32) {
    let leave = leave_mb as usize * MB;
    let ranges = free_ranges(GRANULARITY, TWO_GB);
    let largest = ranges
        .iter()
        .enumerate()
        .max_by_key(|(_, (a, b))| b - a)
        .map(|(i, _)| i);
    let (mut reserved, mut count, mut left) = (0usize, 0usize, 0usize);
    for (i, &(start, stop)) in ranges.iter().enumerate() {
        let start = if Some(i) == largest {
            let keep = (start + leave)
                .min(stop)
                .next_multiple_of(GRANULARITY)
                .min(stop);
            left += keep - start;
            keep
        } else {
            start
        };
        if stop <= start {
            continue;
        }
        // SAFETY: a free range found just now; reserved, never committed.
        let p = unsafe {
            win::VirtualAlloc(
                start as *mut c_void,
                stop - start,
                win::MEM_RESERVE,
                win::PAGE_NOACCESS,
            )
        };
        if !p.is_null() {
            reserved += stop - start;
            count += 1;
        } else {
            left += stop - start;
        }
    }
    if let Ok(mut r) = RESERVED.lock() {
        *r = Some((reserved, count, left));
    }
}

pub fn reserved_line() -> Option<String> {
    let (reserved, count, left) = (*RESERVED.lock().ok()?)?;
    Some(format!(
        "low memory test: reserved {} MB below 2 GB in {count} ranges, {} MB left free there",
        reserved / MB,
        left / MB
    ))
}

/// Committed bytes below and above 2 GB, free bytes below and above.
fn usage() -> [usize; 4] {
    let mut out = [0usize; 4];
    let mut at = GRANULARITY;
    while at < 0xFFFF_0000 {
        let mut info = win::MemoryBasicInformation::default();
        // SAFETY: VirtualQuery only fills `info`.
        let got = unsafe {
            win::VirtualQuery(
                at as *const c_void,
                &mut info,
                std::mem::size_of::<win::MemoryBasicInformation>(),
            )
        };
        if got == 0 || info.region_size == 0 {
            break;
        }
        let high = info.base_address >= TWO_GB;
        match info.state {
            win::MEM_COMMIT => out[usize::from(high)] += info.region_size,
            win::MEM_FREE => out[2 + usize::from(high)] += info.region_size,
            _ => {}
        }
        let end = info.base_address.saturating_add(info.region_size);
        if end <= at {
            break;
        }
        at = end;
    }
    out
}

fn memory_line() -> String {
    let [low, high, free_low, free_high] = usage();
    let mut status = win::MemoryStatusEx {
        length: std::mem::size_of::<win::MemoryStatusEx>() as u32,
        ..Default::default()
    };
    // SAFETY: fills `status`.
    unsafe { win::GlobalMemoryStatusEx(&mut status) };
    let (presents, waited) = crate::pace::stats();
    format!(
        "memory: committed {} MB below 2 GB, {} MB above; free {} MB below, {} MB above; virtual {} of {} MB available; presents {presents}, frame limit waited {waited:.1} s",
        low / MB,
        high / MB,
        free_low / MB,
        free_high / MB,
        status.avail_virtual as usize / MB,
        status.total_virtual as usize / MB,
    )
}

/// Writes a memory line every `seconds` (none for 0) for as long as the
/// game runs (this is the module's own thread).
pub fn watch(seconds: u32) {
    if seconds == 0 {
        return;
    }
    loop {
        crate::log(&memory_line());
        if crate::config().fov_trace {
            crate::log(&crate::fov::trace_line());
        }
        // SAFETY: plain Win32 call.
        unsafe { win::Sleep(seconds * 1000) };
    }
}
