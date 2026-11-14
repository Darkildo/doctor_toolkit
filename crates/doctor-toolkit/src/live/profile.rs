//! The profile of the running game (stage 58, protocol 7) for people and
//! for programs: [`to_text`] (`doctor-toolkit live profile`) and
//! [`to_json`] (`--json`, MCP `live_profile`).

use serde_json::{Value, json};

use super::proto::profile::{IMAGE_CODE, Profile, Stats, TEXTURES, ThreadSamples, format_name};

/// Instruction pointers listed per thread in the text.
pub const TEXT_TOP: usize = 30;

fn ms(us: impl Into<f64>) -> f64 {
    us.into() / 1000.0
}

fn stats_line(s: &Stats) -> String {
    format!(
        "{} samples, mean {:.2} ms, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2} ms",
        s.count,
        s.mean_us / 1000.0,
        ms(s.p50_us),
        ms(s.p95_us),
        ms(s.p99_us),
        ms(s.max_us)
    )
}

fn stats_json(ring: &[u32]) -> Value {
    match Stats::of(ring) {
        Some(s) => json!({
            "count": s.count,
            "per_second": s.per_second(),
            "mean_ms": s.mean_us / 1000.0,
            "min_ms": ms(s.min_us),
            "p50_ms": ms(s.p50_us),
            "p95_ms": ms(s.p95_us),
            "p99_ms": ms(s.p99_us),
            "max_ms": ms(s.max_us),
        }),
        None => Value::Null,
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn thread_label(p: &Profile, thread: u32) -> String {
    match (thread, p.role(thread)) {
        (0, _) => "others".to_owned(),
        (t, Some(role)) => format!("{t:#06x} ({role})"),
        (t, None) => format!("{t:#06x}"),
    }
}

/// The profile as text: frame times, presents, CPU, reads, textures,
/// memory, clock and the sampler (the first [`TEXT_TOP`] pointers per
/// thread).
pub fn to_text(p: &Profile) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    line(format!(
        "module up {:.1} s{}; {} pass(es) of the main loop (thread {}), {} present(s) (thread {})",
        p.uptime_us as f64 / 1e6,
        if p.reset_at_us > 0 {
            format!(", counters since {:.1} s", p.reset_at_us as f64 / 1e6)
        } else {
            String::new()
        },
        p.passes,
        thread_label(p, p.main_thread),
        p.presents,
        thread_label(p, p.render_thread)
    ));
    match Stats::of(&p.pass_us) {
        Some(s) => line(format!(
            "main loop: {:.1} passes/s; pass {}",
            s.per_second(),
            stats_line(&s)
        )),
        None => line("main loop: no passes measured yet".into()),
    }
    if !p.present_hooked {
        line("presents: not measured (the call at 0x0040DFA2 is not hooked)".into());
    } else {
        match Stats::of(&p.present_interval_us) {
            Some(s) => line(format!(
                "presents: {:.1}/s; interval {}",
                s.per_second(),
                stats_line(&s)
            )),
            None => line("presents: none measured yet".into()),
        }
        if let Some(s) = Stats::of(&p.present_us) {
            line(format!(
                "present takes (limiter and Present): {}",
                stats_line(&s)
            ));
        }
    }
    let cpu = |c: Option<super::proto::CpuTime>| match c {
        Some(c) => format!(
            "{:.1} s (user {:.1}, kernel {:.1})",
            c.total_ms() as f64 / 1000.0,
            f64::from(c.user_ms) / 1000.0,
            f64::from(c.kernel_ms) / 1000.0
        ),
        None => "unknown".to_owned(),
    };
    line(format!(
        "CPU: main {}; render {}; process {}",
        cpu(p.main_cpu),
        cpu(p.render_cpu),
        cpu(p.process_cpu)
    ));
    if !p.reads_hooked {
        line("reads: not measured (ReadFile calls not hooked)".into());
    } else if p.io.is_empty() {
        line("reads: none".into());
    } else {
        line("reads by thread:".into());
        for t in &p.io {
            line(format!(
                "  {:<16} {:>7} reads {:>9.1} MB  {:>9.1} ms in ReadFile  mean {:.2} ms  max {:.2} ms  {} slow (>= 4 ms)",
                thread_label(p, t.thread),
                t.reads,
                mb(t.bytes),
                t.total_us as f64 / 1000.0,
                t.mean_us() / 1000.0,
                ms(t.max_us),
                t.slow
            ));
        }
        for s in &p.io_sites {
            line(format!(
                "  call at {:#010x}: {} reads, {:.1} MB, {:.1} ms",
                s.site,
                s.reads,
                mb(s.bytes),
                s.total_us as f64 / 1000.0
            ));
        }
    }
    if !p.slow_reads.is_empty() {
        let skip = p.slow_reads.len().saturating_sub(20);
        line(format!(
            "slow or large reads ({}, the last {} shown):",
            p.slow_reads.len(),
            p.slow_reads.len() - skip
        ));
        for r in &p.slow_reads[skip..] {
            line(format!(
                "  at {:>9.3} s  {:<16} {:>9} bytes  {:.2} ms",
                f64::from(r.at_ms) / 1000.0,
                thread_label(p, r.thread),
                r.bytes,
                ms(r.us)
            ));
        }
    }
    match &p.textures {
        Some(t) => {
            line(format!("textures: {}", t.summary()));
            let formats: Vec<String> = t
                .formats
                .iter()
                .take(8)
                .map(|f| {
                    format!(
                        "{} ({}) {} × {:.1} MB",
                        format_name(f.format),
                        f.format,
                        f.count,
                        mb(f.bytes)
                    )
                })
                .collect();
            if !formats.is_empty() {
                line(format!("  by format: {}", formats.join(", ")));
            }
        }
        None => line("textures: the table could not be read".into()),
    }
    if let Some(m) = &p.memory {
        line(format!(
            "memory: {:.0} MB committed ({:.0} MB above 2 GB), {:.0} MB reserved, {:.0} MB free, largest free {:.0} MB at {:#010x}; {} regions in {} ms{}",
            mb(m.committed),
            mb(m.committed_high),
            mb(m.reserved),
            mb(m.free),
            mb(m.largest_free),
            m.largest_free_at,
            m.regions,
            m.walk_ms,
            if m.finished {
                ""
            } else {
                " (not finished: 2 s)"
            }
        ));
        line(format!(
            "  virtual: {:.0} MB total, {:.0} MB available",
            mb(m.total_virtual),
            mb(m.avail_virtual)
        ));
    }
    let c = &p.clock;
    let opt = |v: Option<String>| v.unwrap_or_else(|| "?".to_owned());
    line(format!(
        "clock: game frame {}, sim time {}, sim {} Hz, fixed step {}, vsync {}, windowed limiter {}, back buffer {}, counter {} Hz",
        opt(c.game_frame.map(|v| v.to_string())),
        opt(c.sim_ms.map(|v| format!("{:.3} s", f64::from(v) / 1000.0))),
        opt(c.sim_hz.map(|v| v.to_string())),
        opt(c.fixed_step.map(|v| v.to_string())),
        opt(c.vsync.map(|v| v.to_string())),
        opt(c.windowed_limiter.map(|v| v.to_string())),
        opt(c.back_buffer.map(|[w, h]| format!("{w}×{h}"))),
        p.qpc_frequency
    ));
    for s in &p.samples {
        samples_text(p, s, &mut line);
    }
    out
}

fn samples_text(p: &Profile, s: &ThreadSamples, line: &mut impl FnMut(String)) {
    let inside = s.in_image();
    let listed: u32 = s.top.iter().map(|e| e.1).sum();
    line(format!(
        "sampler, thread {}: {} samples in {} ms ({} failed, {} dropped, {} places); in godfather2.exe ({:#010x}–{:#010x}) {:.1}%, outside {:.1}%{}",
        thread_label(p, s.thread),
        s.samples,
        s.elapsed_ms,
        s.failed,
        s.dropped,
        s.distinct,
        IMAGE_CODE.0,
        IMAGE_CODE.1,
        s.percent(inside),
        s.percent(listed - inside),
        if listed < s.samples {
            format!(", {:.1}% not listed", s.percent(s.samples - listed))
        } else {
            String::new()
        }
    ));
    for &(eip, n) in s.top.iter().take(TEXT_TOP) {
        line(format!(
            "  {eip:#010x} {:>7} {:>5.1}%{}",
            n,
            s.percent(n),
            if super::proto::profile::in_image(eip) {
                ""
            } else {
                "  (outside)"
            }
        ));
    }
    if !s.callers.is_empty() {
        let attributed: u32 = s.callers.iter().map(|e| e.1).sum();
        line(format!(
            "  outside godfather2.exe by the game's call that led there ({:.1}% of the samples):",
            s.percent(attributed)
        ));
        for &(ret, n) in s.callers.iter().take(TEXT_TOP) {
            line(format!("  {ret:#010x} {:>7} {:>5.1}%", n, s.percent(n)));
        }
    }
}

/// The profile as JSON. `full`: the rings and every sampled pointer;
/// else their statistics and the first 50 pointers per thread (for the
/// answer of a tool).
pub fn to_json(p: &Profile, full: bool) -> Value {
    let ring = |r: &[u32]| {
        let mut v = json!({ "stats": stats_json(r) });
        if full {
            v["us"] = json!(r);
        }
        v
    };
    let cpu = |c: Option<super::proto::CpuTime>| {
        c.map(|c| json!({"user_ms": c.user_ms, "kernel_ms": c.kernel_ms}))
    };
    let top = if full { usize::MAX } else { 50 };
    let image = format!("{:#010x}-{:#010x}", IMAGE_CODE.0, IMAGE_CODE.1);
    json!({
        "uptime_s": p.uptime_us as f64 / 1e6,
        "reset_at_s": p.reset_at_us as f64 / 1e6,
        "qpc_frequency": p.qpc_frequency,
        "main_thread": p.main_thread,
        "render_thread": p.render_thread,
        "present_hooked": p.present_hooked,
        "reads_hooked": p.reads_hooked,
        "passes": p.passes,
        "main_fps": p.main_fps(),
        "pass": ring(&p.pass_us),
        "presents": p.presents,
        "present_fps": p.present_fps(),
        "present_interval": ring(&p.present_interval_us),
        "present_duration": ring(&p.present_us),
        "clock": {
            "game_frame": p.clock.game_frame,
            "sim_ms": p.clock.sim_ms,
            "sim_hz": p.clock.sim_hz,
            "fixed_step": p.clock.fixed_step,
            "vsync": p.clock.vsync,
            "windowed_limiter": p.clock.windowed_limiter,
            "back_buffer": p.clock.back_buffer,
        },
        "cpu": {
            "main": cpu(p.main_cpu),
            "render": cpu(p.render_cpu),
            "process": cpu(p.process_cpu),
        },
        "io": p.io.iter().map(|t| json!({
            "thread": t.thread,
            "role": p.role(t.thread).unwrap_or(if t.thread == 0 { "others" } else { "" }),
            "reads": t.reads,
            "bytes": t.bytes,
            "total_ms": t.total_us as f64 / 1000.0,
            "mean_ms": t.mean_us() / 1000.0,
            "max_ms": ms(t.max_us),
            "slow": t.slow,
        })).collect::<Vec<_>>(),
        "io_sites": p.io_sites.iter().map(|s| json!({
            "site": format!("{:#010x}", s.site),
            "reads": s.reads,
            "bytes": s.bytes,
            "total_ms": s.total_us as f64 / 1000.0,
        })).collect::<Vec<_>>(),
        "slow_reads": p.slow_reads.iter().map(|r| json!({
            "thread": r.thread,
            "bytes": r.bytes,
            "ms": ms(r.us),
            "at_s": f64::from(r.at_ms) / 1000.0,
        })).collect::<Vec<_>>(),
        "textures": p.textures.as_ref().map(|t| json!({
            "entries": TEXTURES,
            "taken": t.taken,
            "created": t.created,
            "render_targets": t.render_targets,
            "cubes": t.cubes,
            "volumes": t.volumes,
            "managed": t.managed,
            "default_pool": t.default_pool,
            "mb": mb(t.bytes),
            "default_mb": mb(t.default_bytes),
            "highest": t.highest,
            "formats": t.formats.iter().map(|f| json!({
                "format": f.format,
                "name": format_name(f.format),
                "count": f.count,
                "mb": mb(f.bytes),
            })).collect::<Vec<_>>(),
            "summary": t.summary(),
        })),
        "memory": p.memory.map(|m| json!({
            "committed_mb": mb(m.committed),
            "committed_above_2gb_mb": mb(m.committed_high),
            "reserved_mb": mb(m.reserved),
            "free_mb": mb(m.free),
            "largest_free_mb": mb(m.largest_free),
            "largest_free_at": format!("{:#010x}", m.largest_free_at),
            "regions": m.regions,
            "finished": m.finished,
            "walk_ms": m.walk_ms,
            "total_virtual_mb": mb(m.total_virtual),
            "avail_virtual_mb": mb(m.avail_virtual),
        })),
        "samples": p.samples.iter().map(|s| {
            let inside = s.in_image();
            let listed: u32 = s.top.iter().map(|e| e.1).sum();
            json!({
                "thread": s.thread,
                "role": p.role(s.thread),
                "samples": s.samples,
                "failed": s.failed,
                "dropped": s.dropped,
                "distinct": s.distinct,
                "elapsed_ms": s.elapsed_ms,
                "image_code": image,
                "in_image_percent": s.percent(inside),
                "outside_percent": s.percent(listed - inside),
                "top": s.top.iter().take(top).map(|&(eip, n)| json!({
                    "eip": format!("{eip:#010x}"),
                    "count": n,
                    "percent": s.percent(n),
                })).collect::<Vec<_>>(),
                "callers": s.callers.iter().take(top).map(|&(ret, n)| json!({
                    "return": format!("{ret:#010x}"),
                    "count": n,
                    "percent": s.percent(n),
                })).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}
