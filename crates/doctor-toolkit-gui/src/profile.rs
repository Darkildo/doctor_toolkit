//! Measurements of the editor: timed scopes, milestones and memory, off
//! unless `DOCTOR_TOOLKIT_GUI_PROFILE` is set (stage 67).
//!
//! `DOCTOR_TOOLKIT_GUI_PROFILE=<file>` (or `-` for standard error) turns
//! the profiler on; the report is written there when the editor exits.
//! With `DOCTOR_TOOLKIT_GUI_PROFILE_UNTIL=<milestone>[+<frames>|+<seconds>s]`
//! the editor writes it and exits by itself after the milestone: that many
//! frames later, drawn one after another (`level+300`), or that many
//! seconds later without asking for frames (`ready+10s`: the frames of
//! an idle editor are then the ones it asks for itself).
//!
//! - [`scope`] times a piece of work: count, total, median, 95th
//!   percentile and maximum per name go to the report.
//! - [`mark`] records a milestone once, with the time since the start of
//!   the process and the resident memory then: `first frame`, `game`
//!   (resources indexed), `index` (the game index with names), `stream`
//!   (the first stream shown), `level` (zones loaded and uploaded),
//!   `ready` (nothing loads any more), and others of the views.
//! - [`frame`] ends a frame of the window: frames, their intervals and
//!   the CPU time of the editor's update go to the report.
//! - [`value`] keeps the last value of a counter (draw calls, GPU time).
//!
//! When the profiler is off every call is one relaxed atomic load.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());

/// Measurements by name.
type Scopes = BTreeMap<&'static str, Vec<Duration>>;

/// When a profiled run ends.
#[derive(Debug, Clone, PartialEq)]
struct Until {
    milestone: String,
    frames: u64,
    seconds: Option<f64>,
}

/// What the window does after a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Next {
    /// Nothing (the profiler is off or waits for a milestone).
    Idle,
    /// Draw the next frame at once.
    Repaint,
    /// Wake up after this long.
    RepaintAfter(Duration),
    /// The run is over: the report is written.
    Exit,
}

#[derive(Default)]
struct State {
    output: Option<String>,
    until: Option<Until>,
    /// Frames and time since the milestone of `until`.
    after: Option<(u64, Instant)>,
    marks: Vec<Mark>,
    scopes: Scopes,
    /// Scopes and frame intervals before the milestone of `until`.
    before: Option<(Scopes, Vec<Duration>)>,
    values: BTreeMap<&'static str, f64>,
    notes: Vec<String>,
    frames: u64,
    last_frame: Option<Instant>,
    intervals: Vec<Duration>,
}

impl State {
    const fn new() -> Self {
        Self {
            output: None,
            until: None,
            after: None,
            marks: Vec::new(),
            scopes: BTreeMap::new(),
            before: None,
            values: BTreeMap::new(),
            notes: Vec::new(),
            frames: 0,
            last_frame: None,
            intervals: Vec::new(),
        }
    }
}

struct Mark {
    name: String,
    at: Duration,
    rss: Option<u64>,
    /// Anonymous resident memory: the heap without mapped files.
    anon: Option<u64>,
    frame: u64,
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the clock and reads the environment; call first thing in `main`.
pub fn init() {
    START.get_or_init(Instant::now);
    let Some(output) = std::env::var("DOCTOR_TOOLKIT_GUI_PROFILE")
        .ok()
        .filter(|v| !v.is_empty())
    else {
        return;
    };
    let until = std::env::var("DOCTOR_TOOLKIT_GUI_PROFILE_UNTIL")
        .ok()
        .map(|spec| parse_until(&spec));
    let mut state = state();
    state.output = Some(output);
    state.until = until;
    ENABLED.store(true, Ordering::Relaxed);
}

/// `<milestone>[+<frames>|+<seconds>s]`.
fn parse_until(spec: &str) -> Until {
    let mut until = Until {
        milestone: spec.to_owned(),
        frames: 0,
        seconds: None,
    };
    if let Some((name, count)) = spec.rsplit_once('+') {
        if let Ok(frames) = count.parse::<u64>() {
            until.milestone = name.to_owned();
            until.frames = frames;
        } else if let Some(Ok(seconds)) = count.strip_suffix('s').map(str::parse::<f64>) {
            until.milestone = name.to_owned();
            until.seconds = Some(seconds);
        }
    }
    until
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn since_start() -> Duration {
    START.get_or_init(Instant::now).elapsed()
}

/// A running measurement; recorded when dropped.
pub struct Scope {
    name: &'static str,
    started: Instant,
}

impl Drop for Scope {
    fn drop(&mut self) {
        record(self.name, self.started.elapsed());
    }
}

/// Times the rest of the enclosing block under `name`.
#[must_use = "the scope measures until it is dropped"]
pub fn scope(name: &'static str) -> Option<Scope> {
    enabled().then(|| Scope {
        name,
        started: Instant::now(),
    })
}

/// Adds one measurement of `name`.
pub fn record(name: &'static str, time: Duration) {
    if enabled() {
        state().scopes.entry(name).or_default().push(time);
    }
}

/// The last value of a counter.
pub fn value(name: &'static str, value: f64) {
    if enabled() {
        state().values.insert(name, value);
    }
}

/// A line of text for the report (adapter, sizes).
pub fn note(text: impl Into<String>) {
    if enabled() {
        state().notes.push(text.into());
    }
}

/// Records milestone `name` the first time it is reached.
pub fn mark(name: &str) {
    if !enabled() {
        return;
    }
    let at = since_start();
    let rss = resident_bytes();
    let anon = status_kb("RssAnon:").map(|kb| kb * 1024);
    let mut state = state();
    if state.marks.iter().any(|m| m.name == name) {
        return;
    }
    let frame = state.frames;
    state.marks.push(Mark {
        name: name.to_owned(),
        at,
        rss,
        anon,
        frame,
    });
    if state.until.as_ref().is_some_and(|u| u.milestone == name) {
        // The steady state after the milestone is measured on its own.
        state.after = Some((0, Instant::now()));
        let scopes = std::mem::take(&mut state.scopes);
        let intervals = std::mem::take(&mut state.intervals);
        state.before = Some((scopes, intervals));
        state.last_frame = None;
    }
}

/// Ends a frame of the window that took `update` of CPU time in the
/// editor's code; says what the window does next. On [`Next::Exit`] the
/// report is written.
pub fn frame(update: Duration) -> Next {
    if !enabled() {
        return Next::Idle;
    }
    let now = Instant::now();
    let next = {
        let mut state = state();
        state.frames += 1;
        if let Some(last) = state.last_frame.replace(now) {
            state.intervals.push(now - last);
        }
        state
            .scopes
            .entry("frame: update")
            .or_default()
            .push(update);
        if state.frames == 1 {
            drop(state);
            mark("first frame");
            state = self::state();
        }
        let until = state.until.clone();
        match (&mut state.after, until) {
            (Some((frames, started)), Some(until)) => {
                *frames += 1;
                match until.seconds {
                    Some(seconds) => {
                        let left = seconds - started.elapsed().as_secs_f64();
                        if left <= 0.0 {
                            Next::Exit
                        } else {
                            Next::RepaintAfter(Duration::from_secs_f64(left))
                        }
                    }
                    None if *frames > until.frames => Next::Exit,
                    None => Next::Repaint,
                }
            }
            _ => Next::Idle,
        }
    };
    if next == Next::Exit {
        mark("end");
        write_report();
    }
    next
}

/// Writes the report to the file of `DOCTOR_TOOLKIT_GUI_PROFILE` (once).
pub fn write_report() {
    if !enabled() {
        return;
    }
    let Some(output) = state().output.take() else {
        return;
    };
    let text = report();
    if output == "-" {
        eprint!("{text}");
    } else if let Err(error) = std::fs::write(&output, text) {
        eprintln!("profile: cannot write {output}: {error}");
    }
}

/// The report as text.
pub fn report() -> String {
    let state = state();
    let mut out = String::new();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mb =
        |b: Option<u64>| b.map_or_else(|| "-".to_owned(), |b| format!("{:.0}", b as f64 / 1e6));
    let _ = writeln!(out, "# doctor-toolkit-gui profile");
    for note in &state.notes {
        let _ = writeln!(out, "note: {note}");
    }
    let _ = writeln!(
        out,
        "## milestones (ms since start, resident MB, of it anonymous MB, frame)"
    );
    for m in &state.marks {
        let _ = writeln!(
            out,
            "{:<24} {:>9.0} {:>7} {:>7} {:>6}",
            m.name,
            ms(m.at),
            mb(m.rss),
            mb(m.anon),
            m.frame
        );
    }
    let _ = writeln!(out, "peak resident MB: {}", mb(peak_resident_bytes()));
    let _ = writeln!(out, "frames: {}", state.frames);
    let mut sections = Vec::new();
    if let Some((scopes, intervals)) = &state.before {
        sections.push(("before the last milestone", scopes, intervals));
        sections.push(("after it", &state.scopes, &state.intervals));
    } else {
        sections.push(("the whole run", &state.scopes, &state.intervals));
    }
    for (title, scopes, intervals) in sections {
        let _ = writeln!(
            out,
            "## {title}: scopes (count, total ms, median, p95, max ms)"
        );
        if !intervals.is_empty() {
            let _ = writeln!(
                out,
                "{:<32} {:>6} {:>9}  {}",
                "frame interval",
                intervals.len(),
                "",
                summary(intervals)
            );
        }
        for (name, times) in scopes {
            let total: Duration = times.iter().sum();
            let _ = writeln!(
                out,
                "{:<32} {:>6} {:>9.1}  {}",
                name,
                times.len(),
                ms(total),
                summary(times)
            );
        }
    }
    if !state.values.is_empty() {
        let _ = writeln!(out, "## last values");
        for (name, value) in &state.values {
            let _ = writeln!(out, "{name:<32} {value:.2}");
        }
    }
    out
}

/// "median a, p95 b, max c" in milliseconds.
pub fn summary(times: &[Duration]) -> String {
    if times.is_empty() {
        return "-".into();
    }
    let mut sorted = times.to_vec();
    sorted.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    format!(
        "median {:.2}, p95 {:.2}, max {:.2}",
        ms(sorted[sorted.len() / 2]),
        ms(sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)]),
        ms(*sorted.last().unwrap())
    )
}

/// Resident memory of the process now (Linux).
pub fn resident_bytes() -> Option<u64> {
    status_kb("VmRSS:").map(|kb| kb * 1024)
}

/// The most resident memory the process had (Linux).
pub fn peak_resident_bytes() -> Option<u64> {
    status_kb("VmHWM:").map(|kb| kb * 1024)
}

fn status_kb(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    line[field.len()..]
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_end_of_a_run() {
        let until = |milestone: &str, frames, seconds| Until {
            milestone: milestone.into(),
            frames,
            seconds,
        };
        assert_eq!(parse_until("ready+300"), until("ready", 300, None));
        assert_eq!(parse_until("ready+10s"), until("ready", 0, Some(10.0)));
        assert_eq!(parse_until("level"), until("level", 0, None));
        assert_eq!(parse_until("first frame"), until("first frame", 0, None));
    }

    #[test]
    fn summarises_times() {
        let times: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(summary(&times), "median 51.00, p95 96.00, max 100.00");
        assert_eq!(summary(&[]), "-");
    }

    #[test]
    fn reads_memory_on_linux() {
        if cfg!(target_os = "linux") {
            assert!(resident_bytes().unwrap() > 0);
            assert!(peak_resident_bytes().unwrap() >= resident_bytes().unwrap() / 2);
        }
    }
}
