//! The graphics module (`doctor_render.asi`, stage 59): what it patches,
//! its configuration and the arithmetic it uses, shared with the editor.
//!
//! The module (crate `doctor-toolkit-render-asi`) is a second ASI module,
//! independent of the live link: it opens no port and answers nothing, it
//! only fixes and improves how the game draws and paces itself
//! (`docs/analysis/performance.md` §8, §10):
//!
//! - **time of day** without losing the rest of every frame
//!   (`0x00826B70`);
//! - **vsync at the refresh rate**: the game's "vertical sync" is a lock at
//!   30 frames a second (`D3DPRESENT_INTERVAL_TWO` full screen, two refreshes
//!   of 16.667 ms in a window); the module makes it one refresh;
//! - the game's own **limiter sleeps** instead of spinning `SleepEx(0)`;
//! - a **frame limit** of its own before `Present`;
//! - the **field of view** of the player's camera;
//! - **post-effects** over the finished frame: FXAA and sharpening (CAS),
//!   instead of the plain copy of the scene into the back buffer;
//! - a **memory test** for Large Address Aware: it reserves the free
//!   addresses below 2 GB so that the game's memory goes above.
//!
//! It checks the build like the live module ([`crate::edition`]) with its
//! own table [`CHECKS`]: the header of PC 1.0.789.0 and the bytes of every
//! place it patches or relies on. None of its patches touches a place the
//! live module checks (a test), so either module may load first.

use crate::edition::Check;

/// File name of the module in the game folder (under `scripts/`).
pub const MODULE_FILE: &str = "doctor_render.asi";
/// File name of its configuration, next to it.
pub const CONFIG_FILE: &str = "doctor_render.cfg";
/// File name of its log, next to it.
pub const LOG_FILE: &str = "doctor_render.log";

/// Addresses of `godfather2.exe` PC 1.0.789.0 the module uses; each is
/// proved by a place in [`CHECKS`].
pub mod addr {
    /// `mov dword [edi], 0` in the time of day's tick `0x00826B70`: zeroes
    /// the counter of 1/1200 s after the whole seconds were given out. Six
    /// bytes, patched to `call hook; nop`.
    pub const TIME_OF_DAY_ZERO: u32 = 0x0082_6BF5;
    /// Units of the counter in one second of the time of day.
    pub const TIME_OF_DAY_UNITS: u32 = 1200;
    /// The immediate `2` of `mov [esi+0x34], 2` (`PresentationInterval =
    /// D3DPRESENT_INTERVAL_TWO`) in the present parameters `0x006073E0`.
    pub const INTERVAL_TWO_IMM: u32 = 0x0060_75E2;
    /// The immediates `2` of `and eax, 2` that give `0x00F15A48` (refreshes
    /// the limiter waits) when vsync is on: device creation and settings.
    pub const VSYNC_WAIT_IMMS: [u32; 2] = [0x0060_7B59, 0x0060_84CE];
    /// `call 0x00608CA0` (the limiter, cdecl, one argument) in the present
    /// `0x0060D8B0`.
    pub const LIMITER_CALL: u32 = 0x0060_D8F2;
    pub const LIMITER: u32 = 0x0060_8CA0;
    /// `call 0x00608E90` (copy of the frame and `Present`) in `0x0060D8B0`.
    pub const PRESENT_CALL: u32 = 0x0060_D8FA;
    pub const PRESENT: u32 = 0x0060_8E90;
    /// `call 0x00608DC0` (copy of the scene into the back buffer) at the
    /// start of `0x00608E90`.
    pub const COPY_CALL: u32 = 0x0060_8E90;
    pub const COPY: u32 = 0x0060_8DC0;
    /// `call 0x00607E10` (release of the targets of the `DEFAULT` pool) in
    /// the reset of the device `0x00608250`.
    pub const RELEASE_CALL: u32 = 0x0060_8257;
    pub const RELEASE: u32 = 0x0060_7E10;
    /// `call 0x00422FB0` (`SetFov` of a view, thiscall, angle and aspect)
    /// in `0x00425EA0`: the camera manager (`0x004252E0`, a list of
    /// cameras of 0x1D0 bytes, angle at `+0x30`) gives the view it draws
    /// with its angle every frame — the only call of `SetFov` that changes
    /// the picture in play (traced on the bench: 60°, then 67.5° at 16:9;
    /// the methods of `0x009BD990` are not called on foot).
    pub const FOV_CALL: u32 = 0x0042_5FE4;
    pub const SET_FOV: u32 = 0x0042_2FB0;
    /// QPC of the last pass of the limiter (`u64`).
    pub const LIMITER_STAMP: u32 = 0x0122_3530;
    /// Byte: 1 — the game waits itself (a window, or no `INTERVAL_TWO`).
    pub const LIMITER_ON: u32 = 0x0120_575A;
    /// `D3DPRESENT_PARAMETERS` of the device (`0x011D9188`): `Windowed` at
    /// `+0x20`, `PresentationInterval` at `+0x34`.
    pub const PRESENT_PARAMS: u32 = 0x011D_9188;
    pub const WINDOWED: u32 = PRESENT_PARAMS + 0x20;
    pub const INTERVAL: u32 = PRESENT_PARAMS + 0x34;
    /// `IDirect3DDevice9*`.
    pub const DEVICE: u32 = 0x0120_5750;
    /// Index of the scene's colour target in the texture table.
    pub const SCENE_TARGET: u32 = 0x0120_58A4;
    /// The texture table: 4096 entries of 0x38 bytes, the
    /// `IDirect3DTexture9*` at `+0x14` (`renderer.md` §6).
    pub const TEXTURES: u32 = 0x011A_0F28;
    pub const TEXTURE_ENTRY: u32 = 0x38;
    pub const TEXTURE_PTR: u32 = 0x14;
    pub const TEXTURE_COUNT: u32 = 0x1000;
    /// The game's vsync setting (byte), read when the device is made.
    pub const VSYNC: u32 = 0x00F1_5A39;
    /// Refreshes of 16.667 ms the limiter waits (`u32`): `vsync ? 2 : 0`.
    pub const VSYNC_WAIT: u32 = 0x00F1_5A48;
    /// The window of the device (`HWND`); `Present` only when set.
    pub const WINDOW: u32 = 0x0120_58B4;
}

/// Places patched by the module: `(address, length)`. Each lies inside a
/// place of [`CHECKS`].
pub const PATCHES: &[(u32, u32)] = &[
    (addr::TIME_OF_DAY_ZERO, 6),
    (addr::INTERVAL_TWO_IMM, 1),
    (addr::VSYNC_WAIT_IMMS[0], 1),
    (addr::VSYNC_WAIT_IMMS[1], 1),
    (addr::LIMITER_CALL + 1, 4),
    (addr::PRESENT_CALL + 1, 4),
    (addr::COPY_CALL + 1, 4),
    (addr::RELEASE_CALL + 1, 4),
    (addr::FOV_CALL + 1, 4),
];

/// Checks the build for the module: the header of PC 1.0.789.0 and
/// [`CHECKS`].
pub fn check(read: &mut dyn FnMut(u32, &mut [u8]) -> bool) -> Result<(), crate::edition::Mismatch> {
    crate::edition::check_with(read, CHECKS)
}

/// [`check`] on the bytes of an executable file.
pub fn check_file(data: &[u8]) -> Result<(), crate::edition::Mismatch> {
    crate::edition::check_file_with(data, CHECKS)
}

/// The configuration: `doctor_render.cfg` next to the module, lines
/// `key=value`, `#` starts a comment, unknown keys are ignored. A missing
/// file or key keeps the default: the fixes that change nothing the player
/// sees on, the rest off.
///
/// ```text
/// time_of_day=1
/// vsync_one=1
/// limiter_sleep=1
/// fps=0
/// fov=0
/// fxaa=0
/// sharpen=0
/// low_memory=0
/// memory_log=0
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    /// The time of day keeps the rest of every frame (`0x00826B70`).
    pub time_of_day: bool,
    /// Vsync waits one refresh, not two: 60 frames a second at 60 Hz
    /// instead of 30.
    pub vsync_one: bool,
    /// The game's limiter (vsync in a window) sleeps instead of spinning.
    pub limiter_sleep: bool,
    /// A frame limit of the module, frames a second; 0 — none.
    pub fps: u32,
    /// Horizontal field of view of the player's camera at 16:9, degrees;
    /// 0 — the game's 60. The vertical angle follows (Hor+ is kept).
    pub fov: u32,
    /// FXAA over the finished frame.
    pub fxaa: bool,
    /// Sharpening (contrast adaptive) over the finished frame, 0–100.
    pub sharpen: u32,
    /// Memory test for Large Address Aware: reserve the free addresses
    /// below 2 GB except this many megabytes, so that the game's memory
    /// goes above; 0 — off.
    pub low_memory: u32,
    /// Seconds between lines about the address space in the log; 0 — none
    /// (with `low_memory`, 10).
    pub memory_log: u32,
    /// Test aid: makes the game's limiter wait as with its vsync setting on
    /// (refreshes to wait `0x00F15A48` = 2, or 1 with `vsync_one`) — the
    /// bench starts in a level without the menus where vsync is chosen,
    /// and the setting of the profile overwrites anything set at load.
    /// Off unless the file says `force_vsync=1`.
    pub force_vsync: bool,
    /// Test aid: logs which calls of `SetFov` (`0x00422FB0`) the game makes
    /// and with what angle. Off unless the file says `fov_trace=1`.
    pub fov_trace: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            time_of_day: true,
            vsync_one: true,
            limiter_sleep: true,
            fps: 0,
            fov: 0,
            fxaa: false,
            sharpen: 0,
            low_memory: 0,
            memory_log: 0,
            force_vsync: false,
            fov_trace: false,
        }
    }
}

/// Frame limits the module accepts.
pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 10..=1000;
/// Fields of view the module accepts, degrees horizontal at 16:9.
pub const FOV_RANGE: std::ops::RangeInclusive<u32> = 40..=120;

impl RenderConfig {
    /// Reads the text of a configuration; bad values keep their defaults,
    /// numbers out of range are clamped.
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let flag = |current: bool| match value {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => current,
            };
            let number = value.parse::<u32>().ok();
            match key.trim() {
                "time_of_day" => out.time_of_day = flag(out.time_of_day),
                "vsync_one" => out.vsync_one = flag(out.vsync_one),
                "limiter_sleep" => out.limiter_sleep = flag(out.limiter_sleep),
                "fps" => {
                    if let Some(n) = number {
                        out.fps = if n == 0 {
                            0
                        } else {
                            n.clamp(*FPS_RANGE.start(), *FPS_RANGE.end())
                        };
                    }
                }
                "fov" => {
                    if let Some(n) = number {
                        out.fov = if n == 0 {
                            0
                        } else {
                            n.clamp(*FOV_RANGE.start(), *FOV_RANGE.end())
                        };
                    }
                }
                "fxaa" => out.fxaa = flag(out.fxaa),
                "sharpen" => {
                    if let Some(n) = number {
                        out.sharpen = n.min(100);
                    }
                }
                "low_memory" => {
                    if let Some(n) = number {
                        out.low_memory = n.min(2047);
                    }
                }
                "memory_log" => {
                    if let Some(n) = number {
                        out.memory_log = n.min(3600);
                    }
                }
                "force_vsync" => out.force_vsync = flag(out.force_vsync),
                "fov_trace" => out.fov_trace = flag(out.fov_trace),
                _ => {}
            }
        }
        out
    }

    /// The text of the file, every key with a comment.
    pub fn to_text(&self) -> String {
        let b = |v: bool| u8::from(v);
        let mut text = format!(
            "# doctor_toolkit graphics module (stage 59): doctor_render.asi reads this file\r\n\
             # time of day without losing the rest of each frame\r\n\
             time_of_day={}\r\n\
             # vsync waits one refresh (60 frames a second at 60 Hz), not two (30)\r\n\
             vsync_one={}\r\n\
             # the game's own limiter sleeps instead of spinning\r\n\
             limiter_sleep={}\r\n\
             # frame limit of the module, frames a second; 0: none\r\n\
             fps={}\r\n\
             # horizontal field of view at 16:9, degrees (40-120); 0: the game's 60\r\n\
             fov={}\r\n\
             # anti-aliasing over the finished frame\r\n\
             fxaa={}\r\n\
             # sharpening over the finished frame, 0-100\r\n\
             sharpen={}\r\n\
             # test of Large Address Aware: megabytes left free below 2 GB; 0: off\r\n\
             low_memory={}\r\n\
             # seconds between lines about memory in doctor_render.log; 0: none\r\n\
             memory_log={}\r\n",
            b(self.time_of_day),
            b(self.vsync_one),
            b(self.limiter_sleep),
            self.fps,
            self.fov,
            b(self.fxaa),
            self.sharpen,
            self.low_memory,
            self.memory_log,
        );
        if self.force_vsync {
            text.push_str(
                "# test: the game's limiter waits as with its vsync on\r\nforce_vsync=1\r\n",
            );
        }
        if self.fov_trace {
            text.push_str("# test: log the calls of SetFov\r\nfov_trace=1\r\n");
        }
        text
    }

    /// Whether the module replaces the copy of the frame with its pass.
    pub fn post(&self) -> bool {
        self.fxaa || self.sharpen > 0
    }

    /// Seconds between memory lines in the log.
    pub fn memory_interval(&self) -> u32 {
        if self.memory_log > 0 {
            self.memory_log
        } else if self.low_memory > 0 {
            10
        } else {
            0
        }
    }

    /// One line for logs and the editor.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.time_of_day {
            parts.push("time of day".to_owned());
        }
        if self.vsync_one {
            parts.push("vsync 1 refresh".to_owned());
        }
        if self.limiter_sleep {
            parts.push("limiter sleeps".to_owned());
        }
        if self.fps > 0 {
            parts.push(format!("{} fps", self.fps));
        }
        if self.fov > 0 {
            parts.push(format!("fov {}°", self.fov));
        }
        if self.fxaa {
            parts.push("FXAA".to_owned());
        }
        if self.sharpen > 0 {
            parts.push(format!("sharpen {}%", self.sharpen));
        }
        if self.low_memory > 0 {
            parts.push(format!("low memory test ({} MB left)", self.low_memory));
        }
        if parts.is_empty() {
            "nothing".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

/// The game's horizontal angle at 16:9 (`renderer.md` §5).
pub const GAME_FOV: f32 = 60.0;

/// The factor on `tan(angle / 2)` that turns the game's angle into `fov`
/// degrees (horizontal at 16:9); `None` for 0 (the game's own).
pub fn fov_factor(fov: u32) -> Option<f32> {
    if fov == 0 {
        return None;
    }
    let half = |deg: f32| (deg.to_radians() / 2.0).tan();
    Some(half(fov as f32) / half(GAME_FOV))
}

/// An angle of the camera (radians) widened by `factor` on the tangent of
/// its half: zooms of the camera (aiming) keep their ratio.
pub fn scale_angle(angle: f32, factor: f32) -> f32 {
    let half = (angle / 2.0).tan() * factor;
    let out = 2.0 * half.atan();
    // Stay a valid projection.
    out.clamp(0.05, 3.0)
}

/// Paces frames at a fixed rate on a clock of `ticks` per second: each
/// frame waits for its deadline, a frame late by more than a period starts
/// the schedule again (no catching up after a stall).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacer {
    period: u64,
    next: u64,
}

impl Pacer {
    pub fn new(ticks_per_second: u64, fps: u32) -> Self {
        Self {
            period: ticks_per_second / u64::from(fps.max(1)),
            next: 0,
        }
    }

    pub fn period(&self) -> u64 {
        self.period
    }

    /// The tick to wait for when a frame is ready at `now`.
    pub fn deadline(&mut self, now: u64) -> u64 {
        let mut deadline = self.next;
        if deadline == 0 || now > deadline.saturating_add(self.period) {
            deadline = now;
        }
        self.next = deadline + self.period;
        deadline
    }
}

/// The places checked, generated from the executable
/// (`docs/analysis/performance.md` §10).
pub const CHECKS: &[Check] = &[
    Check {
        address: 0x00826BCD,
        what: "time of day 0x00826B70: after adding the frame's units (0x00826370) to the counter [edi] (1/1200 s), `counter >= 1200` gives `counter / 1200` whole seconds to 0x00826410 and zeroes the counter at 0x00826BF5 (patched: call hook; nop — keeps the rest)",
        bytes: &[
            0x8B, 0x0F, 0x83, 0xC4, 0x0C, 0x81, 0xF9, 0xB0, 0x04, 0x00, 0x00, 0x72, 0x21, 0xB8,
            0xB5, 0x81, 0x4E, 0x1B, 0xF7, 0xE1, 0xC1, 0xEA, 0x07, 0x52, 0x6A, 0x00, 0x6A, 0x00,
            0x6A, 0x00, 0x6A, 0x00, 0x8D, 0x4E, 0x18, 0xE8, 0x1B, 0xF8, 0xFF, 0xFF, 0xC7, 0x07,
            0x00, 0x00, 0x00, 0x00,
        ],
    },
    Check {
        address: 0x006075D7,
        what: "present parameters 0x006073E0: full screen with vsync takes D3DPRESENT_INTERVAL_TWO at 0x006075DF when the adapter has it (patched: the immediate 2 -> 1, INTERVAL_ONE)",
        bytes: &[
            0x8A, 0x44, 0x24, 0x58, 0xA8, 0x02, 0x74, 0x09, 0xC7, 0x46, 0x34, 0x02, 0x00, 0x00,
            0x00,
        ],
    },
    Check {
        address: 0x00607B40,
        what: "device creation 0x00607990: the vsync setting 0x00F15A39, vsync ? 2 : 0 -> 0x00F15A48, the refreshes of 16.667 ms the limiter waits (patched: and eax, 2 -> 1)",
        bytes: &[
            0xA0, 0x39, 0x5A, 0xF1, 0x00, 0x0F, 0x9D, 0xC1, 0xA2, 0x3B, 0x5A, 0xF1, 0x00, 0xF6,
            0xD8, 0x88, 0x0D, 0x5B, 0x57, 0x20, 0x01, 0x1B, 0xC0, 0x83, 0xE0, 0x02, 0xA3, 0x48,
            0x5A, 0xF1, 0x00,
        ],
    },
    Check {
        address: 0x006084C8,
        what: "settings 0x006082C0: vsync ? 2 : 0 -> 0x00F15A48 (patched: and eax, 2 -> 1)",
        bytes: &[
            0xF6, 0xD8, 0x1B, 0xC0, 0x83, 0xE0, 0x02, 0x80, 0x3D, 0x7F, 0x58, 0x20, 0x01, 0x00,
            0xA3, 0x48, 0x5A, 0xF1, 0x00,
        ],
    },
    Check {
        address: 0x0060D8DA,
        what: "present 0x0060D8B0: limiter 0x00608CA0(*0x00F15A48) when not skipped and vsync is on (patched: call hook), then copy and Present 0x00608E90 (patched: call hook)",
        bytes: &[
            0xA1, 0x48, 0x5A, 0xF1, 0x00, 0x11, 0x15, 0x2C, 0x58, 0x20, 0x01, 0x33, 0xDB, 0x38,
            0x5C, 0x24, 0x10, 0x75, 0x0D, 0x3B, 0xC3, 0x74, 0x09, 0x50, 0xE8, 0xA9, 0xB3, 0xFF,
            0xFF, 0x83, 0xC4, 0x04, 0xE8, 0x91, 0xB5, 0xFF, 0xFF,
        ],
    },
    Check {
        address: 0x00608CA0,
        what: "limiter 0x00608CA0: waits n x 16.667 ms (0x00E4465C) from the last present (QPC at 0x01223530) only when the byte 0x0120575A is set",
        bytes: &[
            0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF8, 0x83, 0xEC, 0x2C, 0x53, 0x33, 0xDB, 0x38, 0x1D,
            0x5A, 0x57, 0x20, 0x01, 0x56, 0x57, 0x0F, 0x84, 0xF4, 0x00, 0x00, 0x00, 0x83, 0x7D,
            0x08, 0x01, 0x0F, 0x8C, 0xEA, 0x00, 0x00, 0x00, 0x8B, 0x3D, 0x10, 0x51, 0xD5, 0x00,
            0x8D, 0x44, 0x24, 0x20, 0x50, 0xFF,
        ],
    },
    Check {
        address: 0x00608DA2,
        what: "limiter 0x00608CA0: its end, the QPC of this present -> 0x01223530",
        bytes: &[
            0x89, 0x15, 0x30, 0x35, 0x22, 0x01, 0x89, 0x35, 0x34, 0x35, 0x22, 0x01,
        ],
    },
    Check {
        address: 0x00608DC0,
        what: "copy of the frame 0x00608DC0: the scene target *0x012058A4 (an entry of the texture table 0x011A0F28, 0x38 bytes, texture at +0x14) -> back buffer of the device *0x01205750, D3DXLoadSurfaceFromSurface",
        bytes: &[
            0xA1, 0xA4, 0x58, 0x20, 0x01, 0x83, 0xEC, 0x08, 0x3D, 0x00, 0x10, 0x00, 0x00, 0x73,
            0x12, 0x8D, 0x0C, 0xC5, 0x00, 0x00, 0x00, 0x00, 0x2B, 0xC8, 0x8D, 0x04, 0xCD, 0x28,
            0x0F, 0x1A, 0x01, 0xEB, 0x02, 0x33, 0xC0, 0x8B, 0x40, 0x14, 0x8D, 0x4C, 0x24, 0x04,
            0x51, 0xC7, 0x44, 0x24, 0x08, 0x00, 0x00, 0x00, 0x00, 0x8B, 0x10, 0x8B, 0x52, 0x48,
            0x6A, 0x00, 0x50, 0xFF, 0xD2, 0xA1, 0x50, 0x57, 0x20, 0x01, 0x8D, 0x14, 0x24, 0x52,
            0x6A, 0x00, 0x6A, 0x00, 0xC7, 0x44, 0x24, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x8B, 0x08,
            0x6A, 0x00, 0x50, 0x8B, 0x41, 0x48, 0xFF, 0xD0, 0x8B, 0x4C, 0x24, 0x04, 0x8B, 0x14,
            0x24, 0x6A, 0x00, 0x6A, 0x01, 0x6A, 0x00, 0x6A, 0x00, 0x51, 0x6A, 0x00, 0x6A, 0x00,
            0x52, 0xE8, 0xBA, 0xE3, 0x72, 0x00, 0x8B, 0x04, 0x24, 0x8B, 0x08, 0x8B, 0x51, 0x08,
            0x50, 0xFF, 0xD2, 0x8B, 0x44, 0x24, 0x04, 0x8B, 0x08, 0x8B, 0x51, 0x08, 0x50, 0xFF,
            0xD2, 0x83, 0xC4, 0x08, 0xC3,
        ],
    },
    Check {
        address: 0x00608E90,
        what: "copy and Present 0x00608E90: copy 0x00608DC0 (patched: call hook), Present when the window 0x012058B4 is set",
        bytes: &[
            0xE8, 0x2B, 0xFF, 0xFF, 0xFF, 0x83, 0x3D, 0xB4, 0x58, 0x20, 0x01, 0x00, 0x74, 0x23,
            0xA1, 0x50, 0x57, 0x20, 0x01, 0x8B, 0x08, 0x8B, 0x51, 0x44, 0x6A, 0x00, 0x6A, 0x00,
            0x6A, 0x00, 0x6A, 0x00, 0x50, 0xFF, 0xD2, 0x3D, 0x68, 0x08, 0x76, 0x88, 0x75, 0x07,
            0xC6, 0x05, 0x5E, 0xDA, 0x19, 0x01, 0x01,
        ],
    },
    Check {
        address: 0x00608250,
        what: "Reset 0x00608250: releases the targets of the DEFAULT pool (0x00607E10, patched: call hook) before IDirect3DDevice9::Reset",
        bytes: &[
            0x51, 0x56, 0x8B, 0x74, 0x24, 0x0C, 0x56, 0xE8, 0xB4, 0xFB, 0xFF, 0xFF,
        ],
    },
    Check {
        address: 0x0060827A,
        what: "Reset 0x00608250: the byte 0x0120575A after Reset — set when windowed (0x011D91A8) or without INTERVAL_TWO (0x011D91BC)",
        bytes: &[
            0xF6, 0x05, 0xBC, 0x91, 0x1D, 0x01, 0x02, 0xC6, 0x05, 0x5A, 0x57, 0x20, 0x01, 0x00,
            0x75, 0x07, 0xC6, 0x05, 0x5A, 0x57, 0x20, 0x01, 0x01,
        ],
    },
    Check {
        address: 0x00425FCF,
        what: "camera manager 0x00425EA0: SetFov 0x00422FB0(angle [ebp+0x10], aspect *0x01205878) of the view it draws with (patched: call hook)",
        bytes: &[
            0xD9, 0x05, 0x78, 0x58, 0x20, 0x01, 0x83, 0xEC, 0x08, 0xD9, 0x5C, 0x24, 0x04, 0x8B,
            0xCF, 0xD9, 0x45, 0x10, 0xD9, 0x1C, 0x24, 0xE8, 0xC7, 0xCF, 0xFF, 0xFF,
        ],
    },
    Check {
        address: 0x00422FB0,
        what: "SetFov 0x00422FB0 (thiscall, angle and aspect on the stack): +0x14C angle, +0x150 aspect",
        bytes: &[
            0xF3, 0x0F, 0x10, 0x44, 0x24, 0x04, 0x57, 0x8B, 0xF9, 0xF3, 0x0F, 0x11, 0x87, 0x4C,
            0x01, 0x00, 0x00, 0xF3, 0x0F, 0x10, 0x44, 0x24, 0x0C, 0xC7, 0x87, 0x40, 0x01, 0x00,
            0x00, 0x01,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip_and_defaults() {
        let c = RenderConfig {
            time_of_day: false,
            vsync_one: true,
            limiter_sleep: false,
            fps: 120,
            fov: 75,
            fxaa: true,
            sharpen: 40,
            low_memory: 256,
            memory_log: 5,
            force_vsync: true,
            fov_trace: true,
        };
        assert_eq!(RenderConfig::parse(&c.to_text()), c);
        assert_eq!(RenderConfig::parse(""), RenderConfig::default());
        assert_eq!(
            RenderConfig::parse(&RenderConfig::default().to_text()),
            RenderConfig::default()
        );
        let d = RenderConfig::default();
        assert!(d.time_of_day && d.vsync_one && d.limiter_sleep);
        assert!(!d.post() && d.fps == 0 && d.fov == 0 && d.memory_interval() == 0);
        let odd = RenderConfig::parse("fps=5\nfov=500\nsharpen=300\nfxaa=maybe # x\nvsync_one=off");
        assert_eq!(odd.fps, 10);
        assert_eq!(odd.fov, 120);
        assert_eq!(odd.sharpen, 100);
        assert!(!odd.fxaa && !odd.vsync_one && odd.post());
        assert_eq!(RenderConfig::parse("low_memory=64").memory_interval(), 10);
    }

    #[test]
    fn field_of_view() {
        assert_eq!(fov_factor(0), None);
        let same = fov_factor(60).unwrap();
        assert!((same - 1.0).abs() < 1e-6);
        // The game's angle at 16:9 becomes the one asked for.
        let k = fov_factor(90).unwrap();
        let a = scale_angle(60f32.to_radians(), k);
        assert!((a.to_degrees() - 90.0).abs() < 0.01, "{}", a.to_degrees());
        // A zoomed angle (aiming) narrows as much as before, in tangents.
        let zoom = scale_angle(30f32.to_radians(), k);
        let ratio = (zoom / 2.0).tan() / (a / 2.0).tan();
        let before = (15f32.to_radians()).tan() / (30f32.to_radians()).tan();
        assert!((ratio - before).abs() < 1e-4);
        assert!(scale_angle(3.1, 10.0) <= 3.0);
    }

    #[test]
    fn pacer_keeps_the_rate_and_restarts_after_a_stall() {
        let mut p = Pacer::new(1_000_000, 100); // 10 000 ticks a frame
        assert_eq!(p.period(), 10_000);
        assert_eq!(p.deadline(5), 5);
        // Ready early: wait for the next deadline.
        assert_eq!(p.deadline(6_000), 10_005);
        // Ready a little late: no wait (deadline already passed), and the
        // schedule keeps its phase.
        assert_eq!(p.deadline(21_000), 20_005);
        assert_eq!(p.deadline(25_000), 30_005);
        // A stall of many frames: start again from now.
        assert_eq!(p.deadline(200_000), 200_000);
        assert_eq!(p.deadline(200_001), 210_000);
    }

    #[test]
    fn checks_fit_and_patches_lie_inside_them() {
        let mut spans: Vec<(u32, u32)> = CHECKS
            .iter()
            .map(|c| (c.address, c.address + c.bytes.len() as u32))
            .collect();
        spans.sort();
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "{:#x} overlaps {:#x}", w[0].0, w[1].0);
        }
        assert!(CHECKS.iter().all(|c| c.bytes.len() <= 256));
        for &(at, len) in PATCHES {
            assert!(
                spans.iter().any(|&(a, b)| a <= at && at + len <= b),
                "{at:#x} is not checked"
            );
        }
        // Calls patched as calls.
        let byte = |va: u32| {
            CHECKS
                .iter()
                .find_map(|c| {
                    let off = va.checked_sub(c.address)? as usize;
                    c.bytes.get(off).copied()
                })
                .unwrap()
        };
        for call in [
            addr::LIMITER_CALL,
            addr::PRESENT_CALL,
            addr::COPY_CALL,
            addr::RELEASE_CALL,
        ]
        .into_iter()
        .chain([addr::FOV_CALL])
        {
            assert_eq!(byte(call), 0xE8, "{call:#x}");
        }
        assert_eq!(byte(addr::INTERVAL_TWO_IMM), 2);
        assert_eq!(byte(addr::VSYNC_WAIT_IMMS[0]), 2);
        assert_eq!(byte(addr::VSYNC_WAIT_IMMS[1]), 2);
        assert_eq!(byte(addr::TIME_OF_DAY_ZERO), 0xC7);
    }

    /// Neither module patches what the other checks: they load in any
    /// order. The live module patches only places whose description says
    /// so.
    #[test]
    fn does_not_collide_with_the_live_module() {
        let overlaps = |a: u32, alen: u32, b: u32, blen: u32| a < b + blen && b < a + alen;
        for c in crate::edition::CHECKS {
            for &(at, len) in PATCHES {
                assert!(
                    !overlaps(at, len, c.address, c.bytes.len() as u32),
                    "{at:#x} patches the live module's check {:#x} ({})",
                    c.address,
                    c.what
                );
            }
            if c.what.contains("patched") || c.what.contains("hook") {
                for mine in CHECKS {
                    assert!(
                        !overlaps(
                            mine.address,
                            mine.bytes.len() as u32,
                            c.address,
                            c.bytes.len() as u32
                        ),
                        "{:#x} checks what the live module patches at {:#x}",
                        mine.address,
                        c.address
                    );
                }
            }
        }
    }

    /// The table matches the real executable (needs the game).
    #[test]
    fn matches_the_game_when_present() {
        let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
            return;
        };
        let path = std::path::Path::new(&dir).join("godfather2.exe");
        let Ok(data) = std::fs::read(&path) else {
            return;
        };
        assert_eq!(check_file(&data), Ok(()));
    }
}
