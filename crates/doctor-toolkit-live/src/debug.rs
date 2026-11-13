//! Live debugging (stage 35): breakpoints, pause and step, the free camera
//! and the replacement of a texture in the running game — what protocol 3
//! adds ([`crate::Request::Break`], [`crate::Request::Control`],
//! [`crate::Request::Debug`], [`crate::Request::SetCamera`],
//! [`crate::Request::Texture`]).
//!
//! **Where the game stops.** The module holds the game's main thread in a
//! safe place until the client says go on (or [`DebugState::hold_limit_s`]
//! passes — the game is never held for good):
//!
//! - [`StopPlace::Frame`] — inside a level frame, before the game's update
//!   (where the module does its writes): a pause, a step, and a break that
//!   was hit while the module itself was running a write;
//! - [`StopPlace::Bus`] — in the message bus, before the message reaches
//!   its listeners: a break on a message;
//! - [`StopPlace::Print`] — in the game's Lua `print`: a break on printed
//!   text (a traced node of a visual script);
//! - [`StopPlace::Lua`] — right after a Lua error.
//!
//! While the game stands, reads are answered (objects, streams, memory);
//! writes and Lua only when it stands in a frame.

use crate::wire::{Reader, Writer};
use crate::{CameraView, DecodeError, Guid};

/// What stops the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakOn {
    /// A message of the bus, before delivery; with `receiver` only when that
    /// object listens to it.
    Message { hash: u32, receiver: Option<Guid> },
    /// Printed text (Lua `print`, `OutputDebugStringA`) that contains this.
    Output { text: String },
    /// Any error of Lua code.
    LuaError,
}

/// A breakpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakRule {
    pub on: BreakOn,
    /// Removed after its first hit.
    pub once: bool,
}

impl BreakRule {
    pub fn message(hash: u32, receiver: Option<Guid>) -> Self {
        Self {
            on: BreakOn::Message { hash, receiver },
            once: false,
        }
    }

    /// Whether a message `hash` delivered to `receivers` hits the rule
    /// (`receivers` is asked only for rules with a receiver).
    pub fn hit_by_message(&self, hash: u32, receivers: &mut dyn FnMut() -> Vec<Guid>) -> bool {
        match &self.on {
            BreakOn::Message { hash: h, receiver } if *h == hash => {
                receiver.is_none_or(|g| receivers().contains(&g))
            }
            _ => false,
        }
    }

    /// Whether printed `text` hits the rule.
    pub fn hit_by_output(&self, text: &str) -> bool {
        matches!(&self.on, BreakOn::Output { text: t } if !t.is_empty() && text.contains(t.as_str()))
    }
}

/// Pause, go on, one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Stop in the next level frame.
    Pause = 1,
    /// Go on.
    Continue = 2,
    /// Go on for one frame and stop again.
    Step = 3,
}

impl Control {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Pause),
            2 => Some(Self::Continue),
            3 => Some(Self::Step),
            _ => None,
        }
    }
}

/// Where the main thread stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopPlace {
    Frame = 1,
    Bus = 2,
    Print = 3,
    Lua = 4,
}

impl StopPlace {
    pub fn from_u8(v: u8) -> Self {
        match v {
            2 => Self::Bus,
            3 => Self::Print,
            4 => Self::Lua,
            _ => Self::Frame,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Bus => "message bus",
            Self::Print => "print",
            Self::Lua => "Lua error",
        }
    }

    /// Whether writes and Lua may run while the game stands here.
    pub fn allows_writes(self) -> bool {
        self == Self::Frame
    }
}

/// Why the game stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Asked to pause.
    Pause,
    /// One frame after a step.
    Step,
    /// A breakpoint on a message: the message and its receivers.
    Message {
        hash: u32,
        data: u32,
        receivers: Vec<Guid>,
    },
    /// A breakpoint on printed text.
    Output {
        text: String,
        script: String,
        line: i32,
    },
    /// A Lua error.
    LuaError { message: String },
}

/// The game stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub reason: StopReason,
    pub place: StopPlace,
    /// Frame of the main loop and game time when it stopped.
    pub frame: u32,
    pub sim_time_ms: u32,
    /// Number the journal's next event will get: what happened before the
    /// stop is numbered below.
    pub seq: u64,
    /// For how long it has stood, and when it goes on by itself.
    pub held_ms: u32,
    pub resume_in_ms: u32,
}

/// Answer to [`crate::Request::Debug`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DebugState {
    pub rules: Vec<BreakRule>,
    /// `Some` while the game stands.
    pub stop: Option<Stop>,
    /// A pause or a step waits for the next level frame.
    pub pending: bool,
    /// Breakpoints hit since the module was loaded.
    pub hits: u32,
    /// Longest stop before the game goes on by itself, seconds.
    pub hold_limit_s: u16,
    /// The camera the module renders with instead of the game's (free
    /// camera), when set.
    pub camera: Option<CameraView>,
}

/// Default and largest [`DebugState::hold_limit_s`].
pub const HOLD_DEFAULT_S: u16 = 120;
pub const HOLD_MAX_S: u16 = 900;
/// Breakpoints a module keeps.
pub const MAX_RULES: usize = 64;

pub(crate) fn put_rules(w: &mut Writer, rules: &[BreakRule]) {
    w.u32(rules.len() as u32);
    for r in rules {
        match &r.on {
            BreakOn::Message { hash, receiver } => {
                w.u8(1);
                w.u32(*hash);
                match receiver {
                    Some(g) => {
                        w.u8(1);
                        w.bytes(g);
                    }
                    None => w.u8(0),
                }
            }
            BreakOn::Output { text } => {
                w.u8(2);
                w.str(text);
            }
            BreakOn::LuaError => w.u8(3),
        }
        w.bool(r.once);
    }
}

pub(crate) fn get_rules(r: &mut Reader<'_>) -> Result<Vec<BreakRule>, DecodeError> {
    let n = r.count(2)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let on = match r.u8()? {
            1 => BreakOn::Message {
                hash: r.u32()?,
                receiver: if r.bool()? { Some(r.guid()?) } else { None },
            },
            2 => BreakOn::Output { text: r.str()? },
            3 => BreakOn::LuaError,
            _ => return Err(DecodeError::BadValue("breakpoint")),
        };
        out.push(BreakRule {
            on,
            once: r.bool()?,
        });
    }
    Ok(out)
}

pub(crate) fn put_camera(w: &mut Writer, c: &Option<CameraView>) {
    match c {
        Some(c) => {
            w.u8(1);
            w.vec3(&c.pose.position);
            w.vec3(&c.pose.right);
            w.vec3(&c.pose.up);
            w.vec3(&c.pose.forward);
            w.opt_f32(c.fov);
        }
        None => w.u8(0),
    }
}

pub(crate) fn get_camera(r: &mut Reader<'_>) -> Result<Option<CameraView>, DecodeError> {
    Ok(if r.bool()? {
        Some(CameraView {
            pose: crate::Pose {
                position: r.vec3()?,
                right: r.vec3()?,
                up: r.vec3()?,
                forward: r.vec3()?,
            },
            fov: r.opt_f32()?,
        })
    } else {
        None
    })
}

pub(crate) fn put_state(w: &mut Writer, s: &DebugState) {
    put_rules(w, &s.rules);
    match &s.stop {
        Some(stop) => {
            w.u8(1);
            match &stop.reason {
                StopReason::Pause => w.u8(1),
                StopReason::Step => w.u8(2),
                StopReason::Message {
                    hash,
                    data,
                    receivers,
                } => {
                    w.u8(3);
                    w.u32(*hash);
                    w.u32(*data);
                    w.u32(receivers.len() as u32);
                    for g in receivers {
                        w.bytes(g);
                    }
                }
                StopReason::Output { text, script, line } => {
                    w.u8(4);
                    w.str(text);
                    w.str(script);
                    w.u32(*line as u32);
                }
                StopReason::LuaError { message } => {
                    w.u8(5);
                    w.str(message);
                }
            }
            w.u8(stop.place as u8);
            w.u32(stop.frame);
            w.u32(stop.sim_time_ms);
            w.u64(stop.seq);
            w.u32(stop.held_ms);
            w.u32(stop.resume_in_ms);
        }
        None => w.u8(0),
    }
    w.bool(s.pending);
    w.u32(s.hits);
    w.u16(s.hold_limit_s);
    put_camera(w, &s.camera);
}

pub(crate) fn get_state(r: &mut Reader<'_>) -> Result<DebugState, DecodeError> {
    let rules = get_rules(r)?;
    let stop = if r.bool()? {
        let reason = match r.u8()? {
            1 => StopReason::Pause,
            2 => StopReason::Step,
            3 => StopReason::Message {
                hash: r.u32()?,
                data: r.u32()?,
                receivers: {
                    let n = r.count(16)?;
                    let mut list = Vec::with_capacity(n);
                    for _ in 0..n {
                        list.push(r.guid()?);
                    }
                    list
                },
            },
            4 => StopReason::Output {
                text: r.str()?,
                script: r.str()?,
                line: r.u32()? as i32,
            },
            5 => StopReason::LuaError { message: r.str()? },
            _ => return Err(DecodeError::BadValue("stop reason")),
        };
        Some(Stop {
            reason,
            place: StopPlace::from_u8(r.u8()?),
            frame: r.u32()?,
            sim_time_ms: r.u32()?,
            seq: r.u64()?,
            held_ms: r.u32()?,
            resume_in_ms: r.u32()?,
        })
    } else {
        None
    };
    Ok(DebugState {
        rules,
        stop,
        pending: r.bool()?,
        hits: r.u32()?,
        hold_limit_s: r.u16()?,
        camera: get_camera(r)?,
    })
}

// --- Textures -----------------------------------------------------------------------

/// Formats of the game's texture table (`0x00F15860`, the index at `+8` of
/// an entry of `0x011A0F28`) whose levels the module can fill: bytes of a
/// block and its side in pixels.
pub fn texture_format(index: u32) -> Option<(u32, u32)> {
    match index {
        // DXT1
        30 | 31 => Some((8, 4)),
        // DXT3, DXT5
        32..=35 => Some((16, 4)),
        // A8R8G8B8
        3 => Some((4, 1)),
        // L8, A8
        8 | 9 => Some((1, 1)),
        // A8L8
        10 => Some((2, 1)),
        _ => None,
    }
}

/// Bytes of all `levels` of a 2D texture `width` × `height` of format
/// `index`, the largest level first (how a `tg4d` resource holds them).
pub fn texture_bytes(index: u32, width: u32, height: u32, levels: u32) -> Option<u64> {
    let (block, side) = texture_format(index)?;
    let mut total = 0u64;
    for l in 0..levels.max(1) {
        let w = (width >> l).max(1);
        let h = (height >> l).max(1);
        let bw = u64::from(w.div_ceil(side));
        let bh = u64::from(h.div_ceil(side));
        total += bw * bh * u64::from(block);
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_hit() {
        let r = BreakRule::message(7, Some([1; 16]));
        assert!(r.hit_by_message(7, &mut || vec![[2; 16], [1; 16]]));
        assert!(!r.hit_by_message(7, &mut || vec![[2; 16]]));
        assert!(!r.hit_by_message(8, &mut || unreachable!()));
        let any = BreakRule::message(7, None);
        assert!(any.hit_by_message(7, &mut || unreachable!()));
        let out = BreakRule {
            on: BreakOn::Output {
                text: "dt:hello:1;".into(),
            },
            once: true,
        };
        assert!(out.hit_by_output("dt:hello:1;"));
        assert!(!out.hit_by_output("dt:hello:12;"));
        assert!(!out.hit_by_message(7, &mut Vec::new));
    }

    #[test]
    fn texture_sizes() {
        // 256 × 256 DXT1 with 9 levels: 32 768 + 8 192 + … + 8 + 8 + 8.
        assert_eq!(texture_bytes(30, 256, 256, 9), Some(43_704));
        // 4 × 4 A8R8G8B8, three levels: 64 + 16 + 4.
        assert_eq!(texture_bytes(3, 4, 4, 3), Some(84));
        // DXT5 2 × 1: one block.
        assert_eq!(texture_bytes(35, 2, 1, 1), Some(16));
        assert_eq!(texture_bytes(99, 4, 4, 1), None);
    }
}
