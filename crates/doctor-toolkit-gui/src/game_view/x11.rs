//! Frames of the nested X server and input into it, over the X protocol
//! (x11rb, pure Rust: no libxcb, no external programs).
//!
//! - **Frames:** a thread waits for DAMAGE on the root window and reads the
//!   screen with `GetImage` (ZPixmap, 32 bits per pixel), at most
//!   [`MAX_FPS`] times a second; only changed screens are read, so the count
//!   of frames is the game's own frame rate.
//! - **Input:** XTest fakes key presses (by keycode of the server's own
//!   keymap), buttons and pointer motion, relative like a mouse (the game
//!   reads relative motion through DirectInput) or absolute.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    self, ConnectionExt as _, CreateWindowAux, ImageFormat, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// Most frames read per second.
pub const MAX_FPS: f32 = 60.0;

/// A window on the host display that is never mapped, for `Xephyr -parent`:
/// it exists as long as this value (and its connection).
pub struct HiddenWindow {
    conn: RustConnection,
    window: u32,
}

impl HiddenWindow {
    /// Makes the window on the display of `$DISPLAY`.
    pub fn create(size: [u32; 2]) -> Result<Self, String> {
        let (conn, screen) = RustConnection::connect(None).map_err(|e| {
            format!("Xephyr needs an X display of the host ($DISPLAY) or Xvfb installed: {e}")
        })?;
        let root = &conn.setup().roots[screen];
        let (root_window, visual) = (root.root, root.root_visual);
        let window = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root_window,
            0,
            0,
            clamp_u16(size[0]),
            clamp_u16(size[1]),
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new(),
        )
        .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        Ok(Self { conn, window })
    }

    pub fn id(&self) -> u32 {
        self.window
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

fn clamp_u16(v: u32) -> u16 {
    v.min(u32::from(u16::MAX)) as u16
}

/// What the frame reader measured.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// Frames read in the last second (screens that changed).
    pub fps: f32,
    /// Time to read and convert one frame, milliseconds.
    pub read_ms: f32,
    pub frames: u64,
}

/// The connection to the game's display: its frames and its input.
pub struct Link {
    input: Input,
    latest: Arc<Mutex<Option<Arc<ColorImage>>>>,
    serial: Arc<AtomicU64>,
    stats: Arc<Mutex<Stats>>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Link {
    /// Connects to `display` (`:N`); frames ask `ctx` to repaint.
    pub fn connect(display: &str, ctx: &egui::Context) -> Result<Self, String> {
        let input = Input::connect(display)?;
        let latest = Arc::new(Mutex::new(None));
        let serial = Arc::new(AtomicU64::new(0));
        let stats = Arc::new(Mutex::new(Stats::default()));
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (display, ctx) = (display.to_owned(), ctx.clone());
            let (latest, serial, stats, error, stop) = (
                Arc::clone(&latest),
                Arc::clone(&serial),
                Arc::clone(&stats),
                Arc::clone(&error),
                Arc::clone(&stop),
            );
            std::thread::Builder::new()
                .name("game-frames".into())
                .spawn(move || {
                    let shared = Shared {
                        latest,
                        serial,
                        stats,
                        stop,
                    };
                    if let Err(e) = read_frames(&display, &ctx, &shared) {
                        *error.lock().expect("error") = Some(e);
                        ctx.request_repaint();
                    }
                })
                .map_err(|e| e.to_string())?
        };
        Ok(Self {
            input,
            latest,
            serial,
            stats,
            error,
            stop,
            thread: Some(thread),
        })
    }

    /// The newest frame and its number (it grows with every frame).
    pub fn frame(&self) -> (u64, Option<Arc<ColorImage>>) {
        (
            self.serial.load(Ordering::Acquire),
            self.latest.lock().expect("frame").clone(),
        )
    }

    pub fn stats(&self) -> Stats {
        *self.stats.lock().expect("stats")
    }

    /// Why frames stopped coming (the server went away).
    pub fn error(&self) -> Option<String> {
        self.error.lock().expect("error").clone()
    }

    pub fn input(&self) -> &Input {
        &self.input
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Shared {
    latest: Arc<Mutex<Option<Arc<ColorImage>>>>,
    serial: Arc<AtomicU64>,
    stats: Arc<Mutex<Stats>>,
    stop: Arc<AtomicBool>,
}

fn read_frames(display: &str, ctx: &egui::Context, shared: &Shared) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| format!("{display}: {e}");
    let (conn, screen) = RustConnection::connect(Some(display)).map_err(|e| err(&e))?;
    let root = conn.setup().roots[screen].clone();
    let (w, h) = (root.width_in_pixels, root.height_in_pixels);
    // DAMAGE tells when the screen changed; without it read at 30 Hz.
    let damage = match conn.damage_query_version(1, 1).map(|c| c.reply()) {
        Ok(Ok(_)) => {
            let id = conn.generate_id().map_err(|e| err(&e))?;
            conn.damage_create(id, root.root, damage::ReportLevel::NON_EMPTY)
                .map_err(|e| err(&e))?;
            conn.flush().map_err(|e| err(&e))?;
            Some(id)
        }
        _ => None,
    };
    let min_interval = Duration::from_secs_f32(1.0 / MAX_FPS);
    let mut dirty = true;
    let mut last_read = Instant::now();
    let mut second = Instant::now();
    let mut logged = Instant::now();
    let mut in_second = 0u32;
    let mut read_ms = 0.0f32;
    let mut pixels: Vec<Color32> = Vec::new();
    while !shared.stop.load(Ordering::Relaxed) {
        match damage {
            Some(_) => {
                while let Some(event) = conn.poll_for_event().map_err(|e| err(&e))? {
                    if let Event::DamageNotify(_) = event {
                        dirty = true;
                    }
                }
            }
            None => dirty = dirty || last_read.elapsed() >= Duration::from_millis(33),
        }
        if second.elapsed() >= Duration::from_secs(1) {
            let mut stats = shared.stats.lock().expect("stats");
            stats.fps = in_second as f32 / second.elapsed().as_secs_f32();
            stats.read_ms = read_ms;
            in_second = 0;
            second = Instant::now();
            ctx.request_repaint();
            // Every 10 s in the log: the frame rate the tab gets.
            if logged.elapsed() >= Duration::from_secs(10) {
                logged = Instant::now();
                doctor_toolkit::report::log(&format!(
                    "game tab: {:.1} frames/s, frame read in {:.1} ms, {} frames",
                    stats.fps, stats.read_ms, stats.frames
                ));
            }
        }
        if !dirty {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }
        dirty = false;
        let started = Instant::now();
        last_read = started;
        if let Some(id) = damage {
            conn.damage_subtract(id, x11rb::NONE, x11rb::NONE)
                .map_err(|e| err(&e))?;
        }
        let image = conn
            .get_image(ImageFormat::Z_PIXMAP, root.root, 0, 0, w, h, !0)
            .map_err(|e| err(&e))?
            .reply()
            .map_err(|e| err(&e))?;
        bgrx_to_pixels(&image.data, usize::from(w) * usize::from(h), &mut pixels)
            .map_err(|e| err(&e))?;
        let frame = Arc::new(ColorImage::new(
            [usize::from(w), usize::from(h)],
            std::mem::take(&mut pixels),
        ));
        *shared.latest.lock().expect("frame") = Some(frame);
        let frames = shared.serial.fetch_add(1, Ordering::AcqRel) + 1;
        let took = started.elapsed();
        read_ms = took.as_secs_f32() * 1000.0;
        in_second += 1;
        shared.stats.lock().expect("stats").frames = frames;
        ctx.request_repaint();
        if took < min_interval {
            std::thread::sleep(min_interval - took);
        }
    }
    Ok(())
}

/// Pixels of a ZPixmap image of depth 24 in 32 bits (bytes B, G, R, X).
pub fn bgrx_to_pixels(data: &[u8], count: usize, out: &mut Vec<Color32>) -> Result<(), String> {
    if data.len() < count * 4 {
        return Err(format!(
            "the screen is not 32 bits per pixel ({} bytes for {count} pixels)",
            data.len()
        ));
    }
    out.clear();
    out.extend(
        data[..count * 4]
            .chunks_exact(4)
            .map(|p| Color32::from_rgb(p[2], p[1], p[0])),
    );
    Ok(())
}

/// Input into the game's display (XTest).
pub struct Input {
    conn: RustConnection,
    root: u32,
    /// Keycode of every keysym of the server's keymap.
    codes: HashMap<u32, u8>,
}

impl Input {
    pub fn connect(display: &str) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| format!("{display}: {e}");
        let (conn, screen) = RustConnection::connect(Some(display)).map_err(|e| err(&e))?;
        conn.xtest_get_version(2, 2)
            .map_err(|e| err(&e))?
            .reply()
            .map_err(|_| format!("{display}: no XTEST extension"))?;
        let root = conn.setup().roots[screen].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let map = conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(|e| err(&e))?
            .reply()
            .map_err(|e| err(&e))?;
        let codes = keycodes(min, map.keysyms_per_keycode, &map.keysyms);
        Ok(Self { conn, root, codes })
    }

    /// Presses or releases the key of `keysym`; `false` when the keymap
    /// has no such key.
    pub fn key(&self, keysym: u32, down: bool) -> bool {
        let Some(&code) = self.codes.get(&keysym) else {
            return false;
        };
        let kind = if down {
            xproto::KEY_PRESS_EVENT
        } else {
            xproto::KEY_RELEASE_EVENT
        };
        self.fake(kind, code, 0, 0, 0)
    }

    pub fn button(&self, button: u8, down: bool) {
        let kind = if down {
            xproto::BUTTON_PRESS_EVENT
        } else {
            xproto::BUTTON_RELEASE_EVENT
        };
        self.fake(kind, button, 0, 0, 0);
    }

    /// Relative pointer motion, like a mouse.
    pub fn motion(&self, dx: i32, dy: i32) {
        let c = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        self.fake(xproto::MOTION_NOTIFY_EVENT, 1, x11rb::NONE, c(dx), c(dy));
    }

    /// Moves the pointer to pixel `x, y` of the screen.
    pub fn warp(&self, x: i32, y: i32) {
        let c = |v: i32| v.clamp(0, i32::from(i16::MAX)) as i16;
        self.fake(xproto::MOTION_NOTIFY_EVENT, 0, self.root, c(x), c(y));
    }

    pub fn flush(&self) {
        let _ = self.conn.flush();
    }

    fn fake(&self, kind: u8, detail: u8, root: u32, x: i16, y: i16) -> bool {
        self.conn
            .xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, root, x, y, 0)
            .is_ok()
    }
}

/// Keycode of every keysym in a keymap (`GetKeyboardMapping` from `min`):
/// unshifted keysyms win over shifted ones, lower keycodes over higher.
pub fn keycodes(min: u8, per_code: u8, keysyms: &[u32]) -> HashMap<u32, u8> {
    let mut codes = HashMap::new();
    let per = usize::from(per_code.max(1));
    for column in 0..per {
        for (i, syms) in keysyms.chunks(per).enumerate() {
            let Some(&sym) = syms.get(column) else {
                continue;
            };
            let Ok(code) = u8::try_from(usize::from(min) + i) else {
                continue;
            };
            if sym != 0 {
                codes.entry(sym).or_insert(code);
            }
        }
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymaps_prefer_unshifted_keys() {
        // Keycodes 8.. with two keysyms each: 8 = a/A, 9 = 1/!, 10 = A/a
        // (a second key that types A first), 11 = Shift_L.
        let syms = [0x61, 0x41, 0x31, 0x21, 0x41, 0x61, 0xffe1, 0];
        let codes = keycodes(8, 2, &syms);
        assert_eq!(codes[&0x61], 8);
        assert_eq!(codes[&0x31], 9);
        assert_eq!(codes[&0x21], 9);
        assert_eq!(codes[&0x41], 10, "A is unshifted on 10");
        assert_eq!(codes[&0xffe1], 11);
        assert!(!codes.contains_key(&0));
    }

    #[test]
    fn bgrx_becomes_rgb() {
        let mut out = Vec::new();
        bgrx_to_pixels(&[1, 2, 3, 0, 10, 20, 30, 255], 2, &mut out).unwrap();
        assert_eq!(
            out,
            vec![Color32::from_rgb(3, 2, 1), Color32::from_rgb(30, 20, 10)]
        );
        assert!(bgrx_to_pixels(&[0; 6], 2, &mut out).is_err());
    }
}
