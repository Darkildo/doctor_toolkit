//! A profile of the running game measured from inside (stage 58,
//! protocol 7): how long the passes of the main loop and the presents of
//! the render thread take, the files it reads and how long they wait, the
//! CPU time of its threads, its textures, its address space, its clock,
//! and where its threads stand (a sampler of their instruction pointers).
//!
//! The module keeps the counters from its start without a lock and
//! without allocating in the hooks (`docs/analysis/live-link.md` §16); a
//! [`crate::Request::Profile`] copies them, and `reset` empties the rings
//! and the counters of reads afterwards. Two profiles a few seconds apart
//! give the rate of frames and of the game's time ([`Profile::frame_rate`],
//! [`Profile::sim_rate`]).

use crate::DecodeError;
use crate::wire::{Reader, Writer};

/// Entries of each ring of durations (the last passes, presents).
pub const RING: usize = 2048;
/// Entries of the ring of slow or large reads.
pub const SLOW_RING: usize = 256;
/// Threads whose reads are counted one by one; the others go together
/// (thread id 0 in [`ThreadIo`]).
pub const IO_THREADS: usize = 16;
/// A read that takes this long (µs) is slow.
pub const SLOW_READ_US: u32 = 4_000;
/// A read of this many bytes is large.
pub const LARGE_READ: u32 = 64 * 1024;
/// Longest sampling a request may ask for (ms).
pub const MAX_SAMPLE_MS: u32 = 10_000;
/// Instruction pointers listed per sampled thread, most frequent first.
pub const TOP_EIPS: usize = 2048;
/// Code of `godfather2.exe`: a sample here stands in the game's own code
/// (outside: Direct3D or its translation, the C library of Windows, Wine,
/// other modules).
pub const IMAGE_CODE: (u32, u32) = (0x0040_1000, 0x00D5_4FFF);
/// Entries of the texture table ([`crate::addr::TEXTURE_TABLE`]).
pub const TEXTURES: usize = 4096;

/// Answer to [`crate::Request::Profile`] (protocol 7).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    /// Microseconds since the module started (its own clock, the
    /// performance counter).
    pub uptime_us: u64,
    /// Ticks of the performance counter per second.
    pub qpc_frequency: u64,
    /// [`Profile::uptime_us`] at the last reset (0: never reset).
    pub reset_at_us: u64,
    /// Thread of the main loop (0: no pass yet).
    pub main_thread: u32,
    /// Thread that presents the frames (0: no present yet).
    pub render_thread: u32,
    /// Whether the present is measured (the call at `0x0040DFA2`).
    pub present_hooked: bool,
    /// Whether the reads of files are measured.
    pub reads_hooked: bool,
    /// Passes of the main loop since the module started.
    pub passes: u64,
    /// The last passes since the reset, oldest first: µs from one pass to
    /// the next.
    pub pass_us: Vec<u32>,
    /// Presents since the module started.
    pub presents: u64,
    /// µs from one present to the next, oldest first.
    pub present_interval_us: Vec<u32>,
    /// µs a present takes (the game's limiter and the device's Present).
    pub present_us: Vec<u32>,
    pub clock: Clock,
    /// CPU time of the main and render threads and of the process.
    pub main_cpu: Option<CpuTime>,
    pub render_cpu: Option<CpuTime>,
    pub process_cpu: Option<CpuTime>,
    /// Reads by thread since the reset (thread 0: the others together).
    pub io: Vec<ThreadIo>,
    /// Reads by patched call of the game since the reset.
    pub io_sites: Vec<SiteIo>,
    /// The last slow or large reads, oldest first.
    pub slow_reads: Vec<SlowRead>,
    /// The texture table (`None`: it could not be read).
    pub textures: Option<TextureUse>,
    /// The address space, when asked for.
    pub memory: Option<MemoryMap>,
    /// The sampler, when asked for: one entry per thread sampled.
    pub samples: Vec<ThreadSamples>,
}

/// Values of the game that decide its pace.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Clock {
    /// Frames the game began (`*0x01129A8C`, `0x0068BAF0`).
    pub game_frame: Option<u32>,
    /// The game's time in ms (`*0x01205224`).
    pub sim_ms: Option<u32>,
    /// Ticks of the simulation per second (`*0x0110AE4C`).
    pub sim_hz: Option<f32>,
    /// Fixed step of the simulation (`*0x0110AE75`).
    pub fixed_step: Option<u8>,
    /// Vertical sync (`*0x00F15A39`).
    pub vsync: Option<u8>,
    /// The limiter of the windowed mode (`*0x0120575A`, `0x00608CA0`).
    pub windowed_limiter: Option<u8>,
    /// Size of the back buffer (`*0x012058B8`, `*0x012058BC`).
    pub back_buffer: Option<[u16; 2]>,
}

/// CPU time of a thread or the process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTime {
    pub user_ms: u32,
    pub kernel_ms: u32,
}

impl CpuTime {
    pub fn total_ms(&self) -> u64 {
        u64::from(self.user_ms) + u64::from(self.kernel_ms)
    }
}

/// Reads of one thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThreadIo {
    /// Thread id; 0 — the threads past the first [`IO_THREADS`].
    pub thread: u32,
    pub reads: u32,
    pub bytes: u64,
    /// Time inside `ReadFile`, µs.
    pub total_us: u64,
    pub max_us: u32,
    /// Reads of [`SLOW_READ_US`] or longer.
    pub slow: u32,
}

impl ThreadIo {
    pub fn mean_us(&self) -> f64 {
        if self.reads == 0 {
            0.0
        } else {
            self.total_us as f64 / f64::from(self.reads)
        }
    }
}

/// Reads through one patched call of the game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SiteIo {
    /// The patched instruction (0: unknown).
    pub site: u32,
    pub reads: u32,
    pub bytes: u64,
    pub total_us: u64,
}

/// One slow or large read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlowRead {
    pub thread: u32,
    pub bytes: u32,
    pub us: u32,
    /// When it ended: ms since the module started.
    pub at_ms: u32,
}

/// The texture table of the renderer (`0x011A0F28`, 4096 entries).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextureUse {
    /// Entries not free.
    pub taken: u32,
    /// Entries with a texture of the device (state 2–4).
    pub created: u32,
    pub render_targets: u32,
    pub cubes: u32,
    pub volumes: u32,
    /// Created ones in the managed pool, in the default pool.
    pub managed: u32,
    pub default_pool: u32,
    /// Estimated bytes of the created textures (levels included; volumes
    /// left out), and of those in the default pool.
    pub bytes: u64,
    pub default_bytes: u64,
    /// Highest index not free (`None`: all free).
    pub highest: Option<u32>,
    /// Created textures by format index (`0x00F15860`), most bytes first.
    pub formats: Vec<FormatUse>,
}

/// Created textures of one format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatUse {
    pub format: u8,
    pub count: u32,
    pub bytes: u64,
}

/// The address space of the game (`VirtualQuery` region by region).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryMap {
    pub committed: u64,
    /// Reserved and not committed.
    pub reserved: u64,
    pub free: u64,
    pub largest_free: u64,
    pub largest_free_at: u32,
    /// Committed above 2 GB.
    pub committed_high: u64,
    pub regions: u32,
    /// Whether the walk reached the top before its time ran out (2 s).
    pub finished: bool,
    pub walk_ms: u32,
    /// `GlobalMemoryStatusEx`: `ullTotalVirtual`, `ullAvailVirtual`.
    pub total_virtual: u64,
    pub avail_virtual: u64,
}

/// Where one thread stood while sampled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadSamples {
    pub thread: u32,
    /// Instruction pointers read.
    pub samples: u32,
    /// Suspends or contexts that failed.
    pub failed: u32,
    /// Samples the table had no room for.
    pub dropped: u32,
    /// Different instruction pointers.
    pub distinct: u32,
    pub elapsed_ms: u32,
    /// `(eip, count)`, most frequent first, at most [`TOP_EIPS`].
    pub top: Vec<(u32, u32)>,
    /// Samples outside the game's code by the game's call that led there:
    /// `(return address, count)` — the first word on the thread's stack
    /// that returns into the game's code right after a call — most
    /// frequent first, at most [`TOP_EIPS`]. Samples with no such word
    /// are not counted here.
    pub callers: Vec<(u32, u32)>,
}

impl ThreadSamples {
    /// Samples of the listed pointers that stand in the game's code.
    pub fn in_image(&self) -> u32 {
        self.top
            .iter()
            .filter(|(eip, _)| in_image(*eip))
            .map(|(_, n)| n)
            .sum()
    }

    /// Share of all samples, in percent.
    pub fn percent(&self, count: u32) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            f64::from(count) * 100.0 / f64::from(self.samples)
        }
    }
}

/// Whether an address lies in the code of `godfather2.exe`.
pub fn in_image(eip: u32) -> bool {
    (IMAGE_CODE.0..=IMAGE_CODE.1).contains(&eip)
}

/// Percentiles of a ring of durations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub count: usize,
    pub mean_us: f64,
    pub min_us: u32,
    pub p50_us: u32,
    pub p95_us: u32,
    pub p99_us: u32,
    pub max_us: u32,
    /// Sum of the ring, µs.
    pub total_us: u64,
}

impl Stats {
    /// Of a ring of durations in µs; `None` when it is empty.
    pub fn of(ring: &[u32]) -> Option<Self> {
        if ring.is_empty() {
            return None;
        }
        let mut sorted = ring.to_vec();
        sorted.sort_unstable();
        let n = sorted.len();
        // Nearest rank.
        let rank = |p: usize| sorted[((p * n).div_ceil(100)).clamp(1, n) - 1];
        let total: u64 = ring.iter().map(|&v| u64::from(v)).sum();
        Some(Self {
            count: n,
            mean_us: total as f64 / n as f64,
            min_us: sorted[0],
            p50_us: rank(50),
            p95_us: rank(95),
            p99_us: rank(99),
            max_us: sorted[n - 1],
            total_us: total,
        })
    }

    /// Events per second over the ring (the ring holds the time from one
    /// event to the next).
    pub fn per_second(&self) -> f64 {
        if self.total_us == 0 {
            0.0
        } else {
            self.count as f64 * 1e6 / self.total_us as f64
        }
    }
}

/// Events per second of a ring of intervals.
pub fn fps(ring: &[u32]) -> Option<f64> {
    Stats::of(ring).map(|s| s.per_second())
}

impl Profile {
    /// Passes of the main loop per second, from its ring.
    pub fn main_fps(&self) -> Option<f64> {
        fps(&self.pass_us)
    }

    /// Presents per second, from their ring.
    pub fn present_fps(&self) -> Option<f64> {
        fps(&self.present_interval_us)
    }

    /// Frames of the game per second between `earlier` and this profile.
    pub fn frame_rate(&self, earlier: &Profile) -> Option<f64> {
        let dt = self.uptime_us.checked_sub(earlier.uptime_us)?;
        let df = self
            .clock
            .game_frame?
            .wrapping_sub(earlier.clock.game_frame?);
        (dt > 0).then(|| f64::from(df) * 1e6 / dt as f64)
    }

    /// Seconds of the game's time per real second between `earlier` and
    /// this profile (1: real time; less: the game falls behind).
    pub fn sim_rate(&self, earlier: &Profile) -> Option<f64> {
        let dt = self.uptime_us.checked_sub(earlier.uptime_us)?;
        let ds = self.clock.sim_ms?.wrapping_sub(earlier.clock.sim_ms?);
        (dt > 0).then(|| f64::from(ds) * 1e3 / dt as f64)
    }

    /// What a thread id is to the game: `main`, `render`, or nothing.
    pub fn role(&self, thread: u32) -> Option<&'static str> {
        if thread == 0 {
            None
        } else if thread == self.main_thread {
            Some("main")
        } else if thread == self.render_thread {
            Some("render")
        } else {
            None
        }
    }
}

/// Name of a format index of the renderer (`0x00F15860`).
pub fn format_name(format: u8) -> &'static str {
    match format {
        3 => "A8R8G8B8",
        8 => "L8",
        9 => "A8",
        10 => "A8L8",
        0x1A => "D24S8",
        30 | 31 => "DXT1",
        32 => "DXT3",
        33..=35 => "DXT5",
        _ => "other",
    }
}

/// Bytes of one level of `w` × `h` texels of a format index.
fn level_bytes(format: u8, w: u64, h: u64) -> u64 {
    let blocks = w.div_ceil(4).max(1) * h.div_ceil(4).max(1);
    match format {
        30 | 31 => blocks * 8,
        32..=35 => blocks * 16,
        8 | 9 => w * h,
        10 => w * h * 2,
        _ => w * h * 4,
    }
}

/// Estimated bytes of a 2D texture with its levels (`levels` 0 counts as
/// one).
pub fn texture_bytes(format: u8, width: u16, height: u16, levels: u8) -> u64 {
    let (mut w, mut h) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let mut sum = 0;
    for _ in 0..levels.max(1) {
        sum += level_bytes(format, w, h);
        if w == 1 && h == 1 {
            break;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    sum
}

impl TextureUse {
    /// Size of one entry of the table.
    pub const ENTRY: usize = 0x38;

    /// Counts a copy of the table (4096 entries of 0x38 bytes; a shorter
    /// copy counts what it holds). Entry: `+0` state (0 free, 1 taken, 2–3
    /// created, 4 locked), `+1` levels, `+2`/`+4` width and height, `+8`
    /// format index, `+0xC` flags (2 cube, 4 volume, 8 render target),
    /// `+0x30` pool (1 managed, 0 default).
    pub fn from_table(table: &[u8]) -> Self {
        let mut out = Self::default();
        let mut formats: Vec<FormatUse> = Vec::new();
        for (i, e) in table.chunks_exact(Self::ENTRY).enumerate() {
            let state = e[0];
            if state == 0 {
                continue;
            }
            out.taken += 1;
            out.highest = Some(i as u32);
            let flags = u32::from_le_bytes([e[0xC], e[0xD], e[0xE], e[0xF]]);
            if flags & 8 != 0 {
                out.render_targets += 1;
            }
            if flags & 2 != 0 {
                out.cubes += 1;
            }
            if flags & 4 != 0 {
                out.volumes += 1;
            }
            if !(2..=4).contains(&state) {
                continue;
            }
            out.created += 1;
            let pool = u32::from_le_bytes([e[0x30], e[0x31], e[0x32], e[0x33]]);
            let managed = pool == 1;
            if managed {
                out.managed += 1;
            } else if pool == 0 {
                out.default_pool += 1;
            }
            if flags & 4 != 0 {
                continue;
            }
            let format = e[8];
            let w = u16::from_le_bytes([e[2], e[3]]);
            let h = u16::from_le_bytes([e[4], e[5]]);
            let mut bytes = texture_bytes(format, w, h, e[1]);
            if flags & 2 != 0 {
                bytes *= 6;
            }
            out.bytes += bytes;
            if pool == 0 {
                out.default_bytes += bytes;
            }
            match formats.iter_mut().find(|f| f.format == format) {
                Some(f) => {
                    f.count += 1;
                    f.bytes += bytes;
                }
                None => formats.push(FormatUse {
                    format,
                    count: 1,
                    bytes,
                }),
            }
        }
        formats.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.format.cmp(&b.format)));
        out.formats = formats;
        out
    }

    /// One line for people.
    pub fn summary(&self) -> String {
        format!(
            "{}/{TEXTURES} entries used, {} created ({} render targets, {} cube, {} volume; {} managed, {} default), about {:.1} MB ({:.1} MB in the default pool), highest index {}",
            self.taken,
            self.created,
            self.render_targets,
            self.cubes,
            self.volumes,
            self.managed,
            self.default_pool,
            self.bytes as f64 / 1_048_576.0,
            self.default_bytes as f64 / 1_048_576.0,
            self.highest
                .map_or_else(|| "none".to_owned(), |h| h.to_string())
        )
    }
}

fn put_ring(w: &mut Writer, ring: &[u32]) {
    w.u32(ring.len() as u32);
    for &v in ring {
        w.u32(v);
    }
}

fn get_ring(r: &mut Reader<'_>) -> Result<Vec<u32>, DecodeError> {
    let n = r.count(4)?;
    (0..n).map(|_| r.u32()).collect()
}

fn put_opt_u8(w: &mut Writer, v: Option<u8>) {
    match v {
        Some(x) => {
            w.u8(1);
            w.u8(x);
        }
        None => w.u8(0),
    }
}

fn get_opt_u8(r: &mut Reader<'_>) -> Result<Option<u8>, DecodeError> {
    Ok(if r.bool()? { Some(r.u8()?) } else { None })
}

fn put_cpu(w: &mut Writer, c: Option<CpuTime>) {
    match c {
        Some(c) => {
            w.u8(1);
            w.u32(c.user_ms);
            w.u32(c.kernel_ms);
        }
        None => w.u8(0),
    }
}

fn get_cpu(r: &mut Reader<'_>) -> Result<Option<CpuTime>, DecodeError> {
    Ok(if r.bool()? {
        Some(CpuTime {
            user_ms: r.u32()?,
            kernel_ms: r.u32()?,
        })
    } else {
        None
    })
}

pub(crate) fn put_profile(w: &mut Writer, p: &Profile) {
    w.u64(p.uptime_us);
    w.u64(p.qpc_frequency);
    w.u64(p.reset_at_us);
    w.u32(p.main_thread);
    w.u32(p.render_thread);
    w.bool(p.present_hooked);
    w.bool(p.reads_hooked);
    w.u64(p.passes);
    put_ring(w, &p.pass_us);
    w.u64(p.presents);
    put_ring(w, &p.present_interval_us);
    put_ring(w, &p.present_us);
    let c = &p.clock;
    w.opt_u32(c.game_frame);
    w.opt_u32(c.sim_ms);
    w.opt_f32(c.sim_hz);
    put_opt_u8(w, c.fixed_step);
    put_opt_u8(w, c.vsync);
    put_opt_u8(w, c.windowed_limiter);
    match c.back_buffer {
        Some([bw, bh]) => {
            w.u8(1);
            w.u16(bw);
            w.u16(bh);
        }
        None => w.u8(0),
    }
    put_cpu(w, p.main_cpu);
    put_cpu(w, p.render_cpu);
    put_cpu(w, p.process_cpu);
    w.u32(p.io.len() as u32);
    for t in &p.io {
        w.u32(t.thread);
        w.u32(t.reads);
        w.u64(t.bytes);
        w.u64(t.total_us);
        w.u32(t.max_us);
        w.u32(t.slow);
    }
    w.u32(p.io_sites.len() as u32);
    for s in &p.io_sites {
        w.u32(s.site);
        w.u32(s.reads);
        w.u64(s.bytes);
        w.u64(s.total_us);
    }
    w.u32(p.slow_reads.len() as u32);
    for s in &p.slow_reads {
        w.u32(s.thread);
        w.u32(s.bytes);
        w.u32(s.us);
        w.u32(s.at_ms);
    }
    match &p.textures {
        Some(t) => {
            w.u8(1);
            w.u32(t.taken);
            w.u32(t.created);
            w.u32(t.render_targets);
            w.u32(t.cubes);
            w.u32(t.volumes);
            w.u32(t.managed);
            w.u32(t.default_pool);
            w.u64(t.bytes);
            w.u64(t.default_bytes);
            w.opt_u32(t.highest);
            w.u32(t.formats.len() as u32);
            for f in &t.formats {
                w.u8(f.format);
                w.u32(f.count);
                w.u64(f.bytes);
            }
        }
        None => w.u8(0),
    }
    match &p.memory {
        Some(m) => {
            w.u8(1);
            w.u64(m.committed);
            w.u64(m.reserved);
            w.u64(m.free);
            w.u64(m.largest_free);
            w.u32(m.largest_free_at);
            w.u64(m.committed_high);
            w.u32(m.regions);
            w.bool(m.finished);
            w.u32(m.walk_ms);
            w.u64(m.total_virtual);
            w.u64(m.avail_virtual);
        }
        None => w.u8(0),
    }
    w.u32(p.samples.len() as u32);
    for s in &p.samples {
        w.u32(s.thread);
        w.u32(s.samples);
        w.u32(s.failed);
        w.u32(s.dropped);
        w.u32(s.distinct);
        w.u32(s.elapsed_ms);
        for list in [&s.top, &s.callers] {
            w.u32(list.len() as u32);
            for &(eip, n) in list {
                w.u32(eip);
                w.u32(n);
            }
        }
    }
}

pub(crate) fn get_profile(r: &mut Reader<'_>) -> Result<Profile, DecodeError> {
    let uptime_us = r.u64()?;
    let qpc_frequency = r.u64()?;
    let reset_at_us = r.u64()?;
    let main_thread = r.u32()?;
    let render_thread = r.u32()?;
    let present_hooked = r.bool()?;
    let reads_hooked = r.bool()?;
    let passes = r.u64()?;
    let pass_us = get_ring(r)?;
    let presents = r.u64()?;
    let present_interval_us = get_ring(r)?;
    let present_us = get_ring(r)?;
    let clock = Clock {
        game_frame: r.opt_u32()?,
        sim_ms: r.opt_u32()?,
        sim_hz: r.opt_f32()?,
        fixed_step: get_opt_u8(r)?,
        vsync: get_opt_u8(r)?,
        windowed_limiter: get_opt_u8(r)?,
        back_buffer: if r.bool()? {
            Some([r.u16()?, r.u16()?])
        } else {
            None
        },
    };
    let main_cpu = get_cpu(r)?;
    let render_cpu = get_cpu(r)?;
    let process_cpu = get_cpu(r)?;
    let n = r.count(32)?;
    let mut io = Vec::with_capacity(n);
    for _ in 0..n {
        io.push(ThreadIo {
            thread: r.u32()?,
            reads: r.u32()?,
            bytes: r.u64()?,
            total_us: r.u64()?,
            max_us: r.u32()?,
            slow: r.u32()?,
        });
    }
    let n = r.count(24)?;
    let mut io_sites = Vec::with_capacity(n);
    for _ in 0..n {
        io_sites.push(SiteIo {
            site: r.u32()?,
            reads: r.u32()?,
            bytes: r.u64()?,
            total_us: r.u64()?,
        });
    }
    let n = r.count(16)?;
    let mut slow_reads = Vec::with_capacity(n);
    for _ in 0..n {
        slow_reads.push(SlowRead {
            thread: r.u32()?,
            bytes: r.u32()?,
            us: r.u32()?,
            at_ms: r.u32()?,
        });
    }
    let textures = if r.bool()? {
        let mut t = TextureUse {
            taken: r.u32()?,
            created: r.u32()?,
            render_targets: r.u32()?,
            cubes: r.u32()?,
            volumes: r.u32()?,
            managed: r.u32()?,
            default_pool: r.u32()?,
            bytes: r.u64()?,
            default_bytes: r.u64()?,
            highest: r.opt_u32()?,
            formats: Vec::new(),
        };
        let n = r.count(13)?;
        for _ in 0..n {
            t.formats.push(FormatUse {
                format: r.u8()?,
                count: r.u32()?,
                bytes: r.u64()?,
            });
        }
        Some(t)
    } else {
        None
    };
    let memory = if r.bool()? {
        Some(MemoryMap {
            committed: r.u64()?,
            reserved: r.u64()?,
            free: r.u64()?,
            largest_free: r.u64()?,
            largest_free_at: r.u32()?,
            committed_high: r.u64()?,
            regions: r.u32()?,
            finished: r.bool()?,
            walk_ms: r.u32()?,
            total_virtual: r.u64()?,
            avail_virtual: r.u64()?,
        })
    } else {
        None
    };
    let n = r.count(32)?;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let mut s = ThreadSamples {
            thread: r.u32()?,
            samples: r.u32()?,
            failed: r.u32()?,
            dropped: r.u32()?,
            distinct: r.u32()?,
            elapsed_ms: r.u32()?,
            ..Default::default()
        };
        for list in [&mut s.top, &mut s.callers] {
            let k = r.count(8)?;
            list.reserve(k);
            for _ in 0..k {
                list.push((r.u32()?, r.u32()?));
            }
        }
        samples.push(s);
    }
    Ok(Profile {
        uptime_us,
        qpc_frequency,
        reset_at_us,
        main_thread,
        render_thread,
        present_hooked,
        reads_hooked,
        passes,
        pass_us,
        presents,
        present_interval_us,
        present_us,
        clock,
        main_cpu,
        render_cpu,
        process_cpu,
        io,
        io_sites,
        slow_reads,
        textures,
        memory,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_of_a_ring() {
        assert_eq!(Stats::of(&[]), None);
        let ring: Vec<u32> = (1..=100).map(|i| i * 100).collect();
        let s = Stats::of(&ring).unwrap();
        assert_eq!(s.count, 100);
        assert_eq!((s.min_us, s.max_us), (100, 10_000));
        assert_eq!((s.p50_us, s.p95_us, s.p99_us), (5_000, 9_500, 9_900));
        assert!((s.mean_us - 5_050.0).abs() < 1e-9);
        // 16 667 µs a frame: 60 frames a second.
        let f = fps(&[16_667; 30]).unwrap();
        assert!((f - 60.0).abs() < 0.01, "{f}");
        let one = Stats::of(&[7]).unwrap();
        assert_eq!((one.p50_us, one.p99_us, one.max_us), (7, 7, 7));
    }

    #[test]
    fn rates_between_two_profiles() {
        let a = Profile {
            uptime_us: 1_000_000,
            clock: Clock {
                game_frame: Some(100),
                sim_ms: Some(5_000),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut b = a.clone();
        b.uptime_us = 3_000_000;
        b.clock.game_frame = Some(160);
        b.clock.sim_ms = Some(6_000);
        assert_eq!(b.frame_rate(&a), Some(30.0));
        assert_eq!(b.sim_rate(&a), Some(0.5));
        assert_eq!(a.frame_rate(&b), None);
        b.clock.sim_ms = None;
        assert_eq!(b.sim_rate(&a), None);
        b.main_thread = 7;
        b.render_thread = 9;
        assert_eq!(b.role(7), Some("main"));
        assert_eq!(b.role(9), Some("render"));
        assert_eq!(b.role(0), None);
    }

    #[allow(clippy::too_many_arguments)]
    fn entry(
        table: &mut [u8],
        i: usize,
        state: u8,
        levels: u8,
        wh: (u16, u16),
        fmt: u8,
        flags: u32,
        pool: u32,
    ) {
        let e = &mut table[i * 0x38..(i + 1) * 0x38];
        e[0] = state;
        e[1] = levels;
        e[2..4].copy_from_slice(&wh.0.to_le_bytes());
        e[4..6].copy_from_slice(&wh.1.to_le_bytes());
        e[8] = fmt;
        e[0xC..0x10].copy_from_slice(&flags.to_le_bytes());
        e[0x30..0x34].copy_from_slice(&pool.to_le_bytes());
    }

    #[test]
    fn texture_table_is_counted() {
        assert_eq!(texture_bytes(3, 4, 4, 1), 64);
        // 256² DXT1 with its 9 levels: 32 768 + 8 192 + … + 8 + 8 + 8.
        assert_eq!(
            texture_bytes(30, 256, 256, 9),
            32_768 + 8_192 + 2_048 + 512 + 128 + 32 + 8 + 8 + 8
        );
        assert_eq!(texture_bytes(33, 1, 1, 0), 16);
        let mut table = vec![0u8; TEXTURES * 0x38];
        entry(&mut table, 0, 2, 1, (64, 64), 3, 0, 1);
        entry(&mut table, 5, 3, 1, (1280, 720), 0x1A, 8, 0);
        entry(&mut table, 9, 2, 1, (32, 32), 32, 2, 1);
        entry(&mut table, 12, 1, 1, (32, 32), 3, 0, 1);
        entry(&mut table, 20, 4, 1, (16, 16), 3, 4, 1);
        let t = TextureUse::from_table(&table);
        assert_eq!(t.taken, 5);
        assert_eq!(t.created, 4);
        assert_eq!((t.render_targets, t.cubes, t.volumes), (1, 1, 1));
        assert_eq!((t.managed, t.default_pool), (3, 1));
        assert_eq!(t.highest, Some(20));
        let rt = 1280 * 720 * 4;
        assert_eq!(t.bytes, 64 * 64 * 4 + rt + 64 * 16 * 6);
        assert_eq!(t.default_bytes, rt);
        assert_eq!(t.formats[0].format, 0x1A);
        assert_eq!(t.formats.len(), 3);
        assert!(t.summary().starts_with("5/4096 entries used, 4 created"));
        assert_eq!(TextureUse::from_table(&[]).highest, None);
    }

    #[test]
    fn samples_in_the_image() {
        let s = ThreadSamples {
            thread: 1,
            samples: 10,
            top: vec![(0x0060_D8B0, 6), (0x7B00_1000, 3)],
            ..Default::default()
        };
        assert_eq!(s.in_image(), 6);
        assert_eq!(s.percent(3), 30.0);
        assert!(in_image(0x0040_1000) && in_image(0x00D5_4FFF));
        assert!(!in_image(0x00D5_5000) && !in_image(0x0040_0FFF));
        assert_eq!(format_name(34), "DXT5");
    }
}
