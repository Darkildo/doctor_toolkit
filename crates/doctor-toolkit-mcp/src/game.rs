//! The game started by `play` (`--allow-game`).
//!
//! On Linux the game runs on an X server of its own — Xvfb, a screen in
//! memory that needs no display of the host and shows nothing anywhere —
//! as the editor's game tab does (stage 33). Its frames are read with
//! `GetImage` for `screenshot`. The game is started in a process group of
//! its own; stopping sends SIGTERM to the group, SIGKILL after
//! [`KILL_AFTER`], and the X server goes after the game. The X server gets
//! SIGKILL when the server's thread that started it ends (also when the
//! server is killed), and without its X server the game under Wine ends
//! too.
//!
//! Elsewhere the game is started as it is (its window shows) and
//! `screenshot` is not available.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use doctor_toolkit::live::{Endpoint, ModuleInstall};
use doctor_toolkit::play::dxvk::DxvkInstall;
use doctor_toolkit::play::start::{CmdFile, StartOptions};
use doctor_toolkit::play::{self, LaunchPlan, Session, SessionState};

/// How long the game may take to end after SIGTERM.
pub const KILL_AFTER: Duration = Duration::from_secs(4);

/// How long an X server may take to start.
const SERVER_START: Duration = Duration::from_secs(15);

/// The X server to run the game on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XServer {
    Xvfb(PathBuf),
    /// A program and its arguments that prints a display number on its
    /// first line (tests).
    Custom(OsString, Vec<OsString>),
}

impl XServer {
    /// `Xvfb` from `PATH` (`DOCTOR_TOOLKIT_NESTED_X` names another Xvfb).
    pub fn find() -> Option<Self> {
        if let Some(chosen) = std::env::var_os("DOCTOR_TOOLKIT_NESTED_X") {
            let path = PathBuf::from(&chosen);
            let path = if path.components().count() > 1 {
                path
            } else {
                play::find_program(&chosen.to_string_lossy())?
            };
            let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
            return name.contains("xvfb").then_some(Self::Xvfb(path));
        }
        play::find_program("Xvfb").map(Self::Xvfb)
    }

    pub fn name(&self) -> String {
        match self {
            Self::Xvfb(_) => "Xvfb".into(),
            Self::Custom(program, _) => program.to_string_lossy().into_owned(),
        }
    }
}

/// A running X server for the game.
#[cfg(unix)]
pub struct Display {
    child: Child,
    number: u32,
    size: [u32; 2],
    server: String,
}

#[cfg(unix)]
impl Display {
    /// Starts `server` with a screen of `size` pixels and waits until it
    /// takes clients.
    pub fn start(server: &XServer, size: [u32; 2]) -> Result<Self, String> {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let [w, h] = size;
        let mut command = match server {
            XServer::Xvfb(path) => {
                let mut c = Command::new(path);
                c.args(["-displayfd", "1", "-nolisten", "tcp", "-noreset"])
                    .args(["-screen", "0", &format!("{w}x{h}x24")]);
                c
            }
            XServer::Custom(program, args) => {
                let mut c = Command::new(program);
                c.args(args);
                c
            }
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        #[cfg(target_os = "linux")]
        {
            // SAFETY: only a prctl system call between fork and exec.
            unsafe {
                command.pre_exec(|| {
                    rustix::process::set_parent_process_death_signal(Some(
                        rustix::process::Signal::KILL,
                    ))
                    .map_err(std::io::Error::from)
                });
            }
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", server.name()))?;
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let number = rx
            .recv_timeout(SERVER_START)
            .ok()
            .and_then(|line| line.trim().parse::<u32>().ok());
        let Some(number) = number else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{} did not report a display within {} s",
                server.name(),
                SERVER_START.as_secs()
            ));
        };
        Ok(Self {
            child,
            number,
            size,
            server: server.name(),
        })
    }

    /// `:N`, the value of `DISPLAY` for the game.
    pub fn name(&self) -> String {
        format!(":{}", self.number)
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn stop(&mut self) {
        if self.alive() {
            signal_group(self.child.id(), rustix::process::Signal::TERM);
            let until = Instant::now() + Duration::from_secs(2);
            while self.alive() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
impl Drop for Display {
    fn drop(&mut self) {
        self.stop();
    }
}

/// No X server of its own outside Unix.
#[cfg(not(unix))]
pub struct Display;

#[cfg(not(unix))]
impl Display {
    pub fn name(&self) -> String {
        String::new()
    }
    pub fn size(&self) -> [u32; 2] {
        [0, 0]
    }
    pub fn server(&self) -> &str {
        ""
    }
    pub fn stop(&mut self) {}
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: rustix::process::Signal) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::kill_process_group(pid, signal).is_ok())
}

/// The game `play` started and what was put into its folder for it.
pub struct GameSession {
    pub started: Instant,
    pub display: Option<Display>,
    session: Session,
    /// Process group (the launched program's id).
    group: u32,
    /// The launched program, killed when nothing else ended it.
    child_pid: Option<u32>,
    pub start: StartOptions,
    pub cmd_file: Option<CmdFile>,
    pub live: Option<ModuleInstall>,
    pub dxvk: Option<DxvkInstall>,
    /// The workspace was written into the game for this launch and is
    /// taken out again when it ends.
    pub revert_after: bool,
    /// What `play` said about the launch.
    pub notes: Vec<String>,
    /// The launch was a stand-in (tests): other programs named like the
    /// game do not count.
    pub own_processes_only: bool,
    exited: bool,
}

impl GameSession {
    /// Starts `plan` on `display` (when given).
    pub fn spawn(plan: &LaunchPlan, display: Option<Display>) -> doctor_toolkit::Result<Self> {
        let mut command = play::command(plan);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            if let Some(d) = &display {
                command
                    .env("DISPLAY", d.name())
                    // Wine's Wayland driver would take a Wayland display first.
                    .env_remove("WAYLAND_DISPLAY")
                    .env_remove("WAYLAND_SOCKET");
            }
            command.process_group(0);
        }
        let child: Child = play::spawn(plan, &mut command)?;
        let group = child.id();
        Ok(Self {
            started: Instant::now(),
            display,
            session: Session::adopt(child, false),
            group,
            child_pid: Some(group),
            start: StartOptions::default(),
            cmd_file: None,
            live: None,
            dxvk: None,
            revert_after: false,
            notes: Vec::new(),
            own_processes_only: false,
            exited: false,
        })
    }

    /// Where the game is.
    pub fn state(&mut self) -> SessionState {
        if self.exited {
            return SessionState::Exited;
        }
        let running = if self.own_processes_only {
            Vec::new()
        } else {
            play::game_processes()
        };
        let state = self.session.poll(&running);
        if matches!(state, SessionState::Exited | SessionState::NotStarted(_)) {
            self.exited = true;
        }
        state
    }

    /// The live link of the module installed for this launch.
    pub fn endpoint(&self) -> Option<Endpoint> {
        self.live.as_ref().map(ModuleInstall::endpoint)
    }

    /// Ends the game: SIGTERM to its process group, SIGKILL after
    /// [`KILL_AFTER`], then its X server. Returns whether it had to be killed.
    pub fn kill(&mut self) -> bool {
        let mut killed = false;
        #[cfg(unix)]
        {
            use rustix::process::Signal;
            if !self.exited {
                signal_group(self.group, Signal::TERM);
                let until = Instant::now() + KILL_AFTER;
                while Instant::now() < until && !matches!(self.state(), SessionState::Exited) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                if !self.exited {
                    killed = true;
                }
            }
            // Whatever is left of the group (Wine's helpers) goes too.
            signal_group(self.group, Signal::KILL);
        }
        #[cfg(not(unix))]
        {
            let _ = self.child_pid;
            killed = !self.exited;
        }
        self.exited = true;
        self.child_pid = None;
        if let Some(mut display) = self.display.take() {
            display.stop();
        }
        killed
    }
}

impl Drop for GameSession {
    fn drop(&mut self) {
        if !self.exited || self.display.is_some() {
            self.kill();
        }
    }
}

/// One frame of the display `:N` as PNG, at most `max_width` pixels wide
/// (halved until it fits). Returns the PNG, its size and the mean
/// brightness (0–255) of the frame.
#[cfg(unix)]
pub fn screenshot(display: &str, max_width: u32) -> Result<(Vec<u8>, [u32; 2], f32), String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

    let (conn, screen) =
        x11rb::connect(Some(display)).map_err(|e| format!("cannot reach {display}: {e}"))?;
    let root = &conn.setup().roots[screen];
    let (w, h) = (root.width_in_pixels, root.height_in_pixels);
    let depth = root.root_depth;
    let bpp = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth)
        .map_or(32, |f| f.bits_per_pixel);
    if bpp != 32 {
        return Err(format!("the display has {bpp} bits per pixel; 32 expected"));
    }
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root.root, 0, 0, w, h, !0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("cannot read the screen: {e}"))?;
    let (w, h) = (u32::from(w), u32::from(h));
    let pixels = (w * h) as usize;
    if reply.data.len() < pixels * 4 {
        return Err(format!("{} bytes for {w}×{h} pixels", reply.data.len()));
    }
    // BGRX, little-endian.
    let mut rgb: Vec<u8> = Vec::with_capacity(pixels * 3);
    for px in reply.data[..pixels * 4].chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let (mut rgb, mut size) = (rgb, [w, h]);
    while size[0] > max_width.max(64) {
        (rgb, size) = halve(&rgb, size);
    }
    let mean = if rgb.is_empty() {
        0.0
    } else {
        rgb.iter().map(|&v| f64::from(v)).sum::<f64>() as f32 / rgb.len() as f32
    };
    Ok((encode_png(&rgb, size)?, size, mean))
}

/// Halves an RGB image, each pixel the mean of a 2×2 block.
pub fn halve(rgb: &[u8], [w, h]: [u32; 2]) -> (Vec<u8>, [u32; 2]) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity((nw * nh * 3) as usize);
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..3 {
                let mut sum = 0u32;
                let mut n = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                    if sx < w && sy < h {
                        sum += u32::from(rgb[((sy * w + sx) * 3 + c) as usize]);
                        n += 1;
                    }
                }
                out.push((sum / n.max(1)) as u8);
            }
        }
    }
    (out, [nw, nh])
}

/// An RGB image as PNG.
pub fn encode_png(rgb: &[u8], [w, h]: [u32; 2]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Standard base64 with padding, for image content.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(ABC[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn halving_averages_blocks() {
        let rgb = [
            0, 0, 0, 100, 100, 100, //
            200, 200, 200, 100, 100, 100,
        ];
        let (out, size) = halve(&rgb, [2, 2]);
        assert_eq!(size, [1, 1]);
        assert_eq!(out, [100, 100, 100]);
        let png = encode_png(&out, size).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
    }
}
