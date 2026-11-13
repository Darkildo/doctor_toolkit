//! `trinity_seq_master` resources: the in-engine cut-scene shots ("Trinity"
//! sequences).
//!
//! One resource is one shot. It is a memory image of the sequence the tool
//! chain compiled from an XML file (`x:\assets\igcs\igc01\source\igc01_s01.xml`),
//! and its resource id is the SDBM of that path in lower case. The shot is a
//! list of commands the sequencer (`godfather2.exe` 0x00599720) executes in
//! order: a `Wait` command suspends the list for a number of seconds, every
//! other command runs at once. Handlers are looked up by the command code in
//! a table of 41 entries at 0x00E2BCE0.
//!
//! ```text
//! seqm (header, 0x520 bytes)
//! 0x000  "seqm"
//! 0x004  char[260]  source XML path
//! 0x108  u32  size of the resource
//! 0x10C  u32  number of blocks (always 1)
//! 0x110  u32  offset of the block table (0x120); 12 bytes of 0xBB follow
//! 0x120  256 × u32  block offsets, unused slots 0xBBBBBBBB
//! seqb (block)
//! 0x00  "seqb"
//! 0x04  u32  version = 4
//! 0x08  u32  hash (unknown source)
//! 0x0C  9 × f32  rotation (x, y, z, w), position, 1, 0
//! 0x30  char[128]  sequence name (`trSeq_igc01_s01`)
//! 0xB0  u32  size of the block
//! 0xB4  f32  duration, seconds
//! 0xB8  u32  offset of the first command (0xC0); 4 bytes 0xBB follow
//! 0xC0  commands: u8 operation, u8 subsystem, u16 size with this header
//! ```
//!
//! Text fields are fixed buffers; the bytes after the terminating zero are
//! whatever the tool's memory held (0xBB fill or old text) and are kept so
//! that the resource is written back byte for byte.

use serde::{Deserialize, Serialize};

use crate::hash::sdbm;
use crate::{Error, Result};

/// Type id of `trinity_seq_master` resources.
pub const TYPE_TRINITY_SEQ_MASTER: u32 = sdbm(b"trinity_seq_master");

const WHAT: &str = "trinity_seq_master";
const HEADER_SIZE: usize = 0x520;
const BLOCK_TABLE: usize = 0x120;
const BLOCK_SLOTS: usize = 256;
const BLOCK_HEADER: usize = 0xC0;
const PATH_LEN: usize = 260;
const NAME_LEN: usize = 128;
const FILL: u8 = 0xBB;

/// Frame rate the tangents of curve keys refer to.
pub const CURVE_FRAME_RATE: f32 = 30.0;

/// Command codes: subsystem in the high byte, operation in the low byte.
pub mod code {
    pub const END: u16 = 0x100;
    pub const WAIT: u16 = 0x101;
    pub const WAIT_ACTOR: u16 = 0x102;
    pub const ACTOR_CREATE: u16 = 0x200;
    pub const ACTOR_ANIM: u16 = 0x205;
    pub const ACTOR_TRANSFORM: u16 = 0x206;
    pub const CAMERA_CREATE: u16 = 0x300;
    pub const CAMERA_CURVES: u16 = 0x304;
    pub const CAMERA_FOV: u16 = 0x307;
    pub const SCRIPT: u16 = 0x402;
    pub const FADE: u16 = 0x501;
    pub const VFX_CREATE: u16 = 0x600;
    pub const VFX_TRANSFORM: u16 = 0x601;
    pub const VFX_STATE: u16 = 0x602;
    pub const VFX_ATTACH: u16 = 0x603;
    pub const VFX_CURVES: u16 = 0x604;
}

/// A 128-bit GUID of a SimGroup object, written as 32 hexadecimal digits of
/// its bytes (as `logic-export` writes object GUIDs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Guid(pub [u8; 16]);

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for Guid {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bytes = parse_hex(s)?;
        let array: [u8; 16] = bytes
            .try_into()
            .map_err(|_| Error::InvalidArgument(format!("GUID `{s}`: 32 hex digits expected")))?;
        Ok(Self(array))
    }
}

impl Serialize for Guid {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Guid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(Error::InvalidArgument(format!(
            "`{s}`: odd number of hex digits"
        )));
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|_| Error::InvalidArgument(format!("`{s}` is not hexadecimal")))
        })
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

mod hex_bytes {
    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(v))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        super::parse_hex(&s).map_err(serde::de::Error::custom)
    }
}

mod hex_opt {
    pub fn serialize<S: serde::Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_str(&super::to_hex(v)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        let s = <Option<String> as serde::Deserialize>::deserialize(d)?;
        s.map(|s| super::parse_hex(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

/// Text in a fixed buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FixedText {
    /// The text up to the terminating zero (bytes as Latin-1 characters).
    pub text: String,
    /// The bytes after the zero when they differ from the usual fill; written
    /// back only while they still fit the buffer exactly.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "hex_opt")]
    pub slack: Option<Vec<u8>>,
}

impl FixedText {
    /// Text without recorded slack bytes.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            slack: None,
        }
    }

    fn read(field: &[u8], fill: u8) -> Self {
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        let text = field[..end].iter().map(|&b| b as char).collect();
        let rest = field.get(end + 1..).unwrap_or(&[]);
        let slack = (!rest.iter().all(|&b| b == fill)).then(|| rest.to_vec());
        Self { text, slack }
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        self.text
            .chars()
            .map(|c| {
                u8::try_from(u32::from(c))
                    .ok()
                    .filter(|&b| b != 0)
                    .ok_or_else(|| {
                        Error::InvalidArgument(format!(
                            "`{}`: character {c:?} cannot be stored",
                            self.text
                        ))
                    })
            })
            .collect()
    }

    /// The text as it is stored, in a buffer of `len` bytes.
    fn write(&self, out: &mut Vec<u8>, len: usize, fill: u8) -> Result<()> {
        let bytes = self.bytes()?;
        if bytes.len() > len {
            return Err(Error::InvalidArgument(format!(
                "`{}` is longer than {len} bytes",
                self.text
            )));
        }
        out.extend_from_slice(&bytes);
        if bytes.len() == len {
            return Ok(());
        }
        out.push(0);
        let rest = len - bytes.len() - 1;
        match &self.slack {
            Some(slack) if slack.len() == rest => out.extend_from_slice(slack),
            _ => out.extend(std::iter::repeat_n(fill, rest)),
        }
        Ok(())
    }
}

/// A key of a curve (subsystem curves of cameras and effects): a cubic
/// Bézier segment to the next key. Tangents are angles (radians) of the
/// slope in value per frame at [`CURVE_FRAME_RATE`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct CurveKey {
    /// Seconds from the start of the command.
    pub time: f32,
    pub in_tangent: f32,
    pub out_tangent: f32,
    /// Tangent weights; the game does not use them.
    pub in_weight: f32,
    pub out_weight: f32,
    pub value: f32,
}

impl CurveKey {
    /// A key with flat tangents and unit weights.
    pub fn flat(time: f32, value: f32) -> Self {
        Self {
            time,
            in_tangent: 0.0,
            out_tangent: 0.0,
            in_weight: 1.0,
            out_weight: 1.0,
            value,
        }
    }
}

/// Value of a curve at `t` seconds (0x00595610): Bézier between keys with
/// control points `v0 + tan(out0)·Δt·10` and `v1 − tan(in1)·Δt·10`; before
/// the first key the first value, after the last key the last value.
pub fn eval_curve(keys: &[CurveKey], t: f32) -> f32 {
    let Some(first) = keys.first() else {
        return 0.0;
    };
    if keys.len() == 1 || t <= first.time {
        return first.value;
    }
    let Some(i) = keys.iter().position(|k| k.time > t) else {
        return keys[keys.len() - 1].value;
    };
    let (k0, k1) = (&keys[i - 1], &keys[i]);
    let dt = k1.time - k0.time;
    if dt <= 0.0 {
        return k1.value;
    }
    let s = (t - k0.time) / dt;
    let scale = dt * CURVE_FRAME_RATE / 3.0;
    let p1 = k0.value + k0.out_tangent.tan() * scale;
    let p2 = k1.value - k1.in_tangent.tan() * scale;
    let u = 1.0 - s;
    u * u * u * k0.value + 3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s * k1.value
}

/// A position key: seconds from the start of the command and a point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct PositionKey {
    pub time: f32,
    pub position: [f32; 3],
}

/// A rotation key: seconds from the start of the command and a quaternion
/// `(x, y, z, w)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct RotationKey {
    pub time: f32,
    pub rotation: [f32; 4],
}

/// Position at `t`, linear between keys (0x00595950).
pub fn eval_positions(keys: &[PositionKey], t: f32) -> Option<[f32; 3]> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.time {
        return Some(first.position);
    }
    let Some(i) = keys.iter().position(|k| k.time > t) else {
        return keys.last().map(|k| k.position);
    };
    let (a, b) = (&keys[i - 1], &keys[i]);
    let s = ((t - a.time) / (b.time - a.time)).clamp(0.0, 1.0);
    Some(std::array::from_fn(|k| {
        a.position[k] + (b.position[k] - a.position[k]) * s
    }))
}

/// Rotation at `t`, normalised linear blend between keys.
pub fn eval_rotations(keys: &[RotationKey], t: f32) -> Option<[f32; 4]> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.time {
        return Some(first.rotation);
    }
    let Some(i) = keys.iter().position(|k| k.time > t) else {
        return keys.last().map(|k| k.rotation);
    };
    let (a, b) = (&keys[i - 1], &keys[i]);
    let s = ((t - a.time) / (b.time - a.time)).clamp(0.0, 1.0);
    let dot: f32 = (0..4).map(|k| a.rotation[k] * b.rotation[k]).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let q: [f32; 4] = std::array::from_fn(|k| a.rotation[k] * (1.0 - s) + b.rotation[k] * sign * s);
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    Some(q.map(|v| v / len))
}

/// Camera or effect curves: position x, y, z and rotation x, y, z in degrees
/// (`R = Rz · Ry · Rx`, the camera looks down −Z with Y up).
pub type Curves = [Vec<CurveKey>; 6];

/// One command of a shot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// `0x101`: suspends the list for this many seconds (relative to the
    /// previous wait).
    Wait { seconds: f32 },
    /// `0x102`: suspends the list until an actor reaches a state (condition 0
    /// or 1; not used by the game's shots).
    WaitActor { actor: Guid, condition: u32 },
    /// `0x200`: creates an actor for the SimGroup object `actor` (an
    /// `Animated` or `PartedAnimated` of the shot's stream).
    ActorCreate {
        actor: Guid,
        name: FixedText,
        /// Three words at 0x34 (0 in every shot; the last two are flags).
        flags: [u32; 3],
        position: [f32; 3],
        reserved: u32,
    },
    /// `0x205`: plays an animation clip (SDBM of `clip` in a `bnk` of the
    /// shot's stream).
    ActorAnim {
        actor: Guid,
        clip: FixedText,
        /// f32 at 0x54 (1.0 in every shot).
        speed: f32,
        /// Words at 0x58–0x68: a flag, then four numbers (0, 120 or 1384,
        /// 8, 1, 0 in the game's shots).
        params: [u32; 5],
    },
    /// `0x206`: places an actor (position and rotation keys).
    ActorTransform {
        actor: Guid,
        positions: Vec<PositionKey>,
        rotations: Vec<RotationKey>,
    },
    /// `0x300`: creates camera `camera`.
    CameraCreate {
        camera: u32,
        flags: u32,
        aspect: f32,
    },
    /// `0x304`: camera path.
    CameraCurves { camera: u32, curves: Curves },
    /// `0x307`: camera vertical field of view in degrees.
    CameraFov { camera: u32, keys: Vec<CurveKey> },
    /// `0x402`: a text command: kind 0 — time of day (`changeTime`,
    /// `changeApparentTime`, `queueChangeTime`, `restoreTimeState`), 1 — a
    /// message on the game's message bus, 2 — shadow ranges
    /// (`setShadowRanges`, `restoreShadowRanges`), 3 — `setTimeScale`
    /// (handler 0x00845A50).
    Script {
        kind: u8,
        /// Size of the text buffer.
        capacity: u8,
        text: FixedText,
        trailer: u8,
    },
    /// `0x501`: screen fade: mode (0 or 1), seconds, colour RGBA.
    Fade {
        mode: u32,
        seconds: f32,
        color: [f32; 4],
    },
    /// `0x600`: creates effect `vfx` from a `vfx` resource.
    VfxCreate {
        vfx: u32,
        /// GUID of the `vfx` resource (its id four times).
        resource: [u32; 4],
        name: FixedText,
        position: [f32; 3],
    },
    /// `0x601`: places an effect (position and rotation keys).
    VfxTransform {
        vfx: u32,
        positions: Vec<PositionKey>,
        rotations: Vec<RotationKey>,
    },
    /// `0x602`: effect state: 0 starts it, 2 stops it at the end of the
    /// frame; 1, 3 and 4 are other calls of the effect.
    VfxState { vfx: u32, state: u32 },
    /// `0x603`: attaches an effect to a bone of an actor.
    VfxAttach {
        vfx: u32,
        actor: Guid,
        bone: FixedText,
        offset: [f32; 3],
    },
    /// `0x604`: effect path.
    VfxCurves { vfx: u32, curves: Curves },
    /// Any other command, kept as it is.
    Raw {
        code: u16,
        #[serde(with = "hex_bytes")]
        data: Vec<u8>,
    },
}

impl Command {
    /// The command code.
    pub fn code(&self) -> u16 {
        match self {
            Self::Wait { .. } => code::WAIT,
            Self::WaitActor { .. } => code::WAIT_ACTOR,
            Self::ActorCreate { .. } => code::ACTOR_CREATE,
            Self::ActorAnim { .. } => code::ACTOR_ANIM,
            Self::ActorTransform { .. } => code::ACTOR_TRANSFORM,
            Self::CameraCreate { .. } => code::CAMERA_CREATE,
            Self::CameraCurves { .. } => code::CAMERA_CURVES,
            Self::CameraFov { .. } => code::CAMERA_FOV,
            Self::Script { .. } => code::SCRIPT,
            Self::Fade { .. } => code::FADE,
            Self::VfxCreate { .. } => code::VFX_CREATE,
            Self::VfxTransform { .. } => code::VFX_TRANSFORM,
            Self::VfxState { .. } => code::VFX_STATE,
            Self::VfxAttach { .. } => code::VFX_ATTACH,
            Self::VfxCurves { .. } => code::VFX_CURVES,
            Self::Raw { code, .. } => *code,
        }
    }

    /// Short name for lists.
    pub fn label(&self) -> String {
        match self {
            Self::Wait { seconds } => format!("wait {seconds:.3} s"),
            Self::WaitActor { actor, .. } => format!("wait for actor {actor}"),
            Self::ActorCreate { name, .. } => format!("actor {}", name.text),
            Self::ActorAnim { clip, .. } => format!("anim {}", clip.text),
            Self::ActorTransform { .. } => "place actor".to_owned(),
            Self::CameraCreate { camera, .. } => format!("camera {camera}"),
            Self::CameraCurves { camera, .. } => format!("camera {camera} path"),
            Self::CameraFov { camera, .. } => format!("camera {camera} field of view"),
            Self::Script { text, .. } => text.text.clone(),
            Self::Fade { mode, seconds, .. } => format!("fade {mode} {seconds} s"),
            Self::VfxCreate { name, .. } => format!("effect {}", name.text),
            Self::VfxTransform { vfx, .. } => format!("place effect {vfx:08x}"),
            Self::VfxState { vfx, state } => format!("effect {vfx:08x} state {state}"),
            Self::VfxAttach { bone, .. } => format!("attach effect to {}", bone.text),
            Self::VfxCurves { vfx, .. } => format!("effect {vfx:08x} path"),
            Self::Raw { code, data } => format!("command {code:04x} ({} bytes)", data.len()),
        }
    }

    /// The actor the command refers to.
    pub fn actor(&self) -> Option<Guid> {
        match self {
            Self::WaitActor { actor, .. }
            | Self::ActorCreate { actor, .. }
            | Self::ActorAnim { actor, .. }
            | Self::ActorTransform { actor, .. }
            | Self::VfxAttach { actor, .. } => Some(*actor),
            _ => None,
        }
    }

    /// The effect the command refers to.
    pub fn vfx(&self) -> Option<u32> {
        match self {
            Self::VfxCreate { vfx, .. }
            | Self::VfxTransform { vfx, .. }
            | Self::VfxState { vfx, .. }
            | Self::VfxAttach { vfx, .. }
            | Self::VfxCurves { vfx, .. } => Some(*vfx),
            _ => None,
        }
    }
}

/// One block (`seqb`): a named list of commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeqBlock {
    pub version: u32,
    pub hash: u32,
    /// Rotation (x, y, z, w), position, then 1 and 0.
    pub transform: [f32; 9],
    pub name: FixedText,
    /// Duration in seconds.
    pub duration: f32,
    pub commands: Vec<Command>,
}

/// A parsed `trinity_seq_master` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeqMaster {
    /// The XML file the shot was compiled from.
    pub path: FixedText,
    pub blocks: Vec<SeqBlock>,
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let out = self
            .at
            .checked_add(n)
            .and_then(|end| self.data.get(self.at..end))
            .ok_or_else(|| Error::malformed(WHAT, self.at, format!("{n} bytes past the end")))?;
        self.at += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn guid(&mut self) -> Result<Guid> {
        Ok(Guid(self.bytes(16)?.try_into().expect("16 bytes")))
    }
    fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn vec4(&mut self) -> Result<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
    fn text(&mut self, len: usize, fill: u8) -> Result<FixedText> {
        Ok(FixedText::read(self.bytes(len)?, fill))
    }
    fn curve_key(&mut self) -> Result<CurveKey> {
        Ok(CurveKey {
            time: self.f32()?,
            in_tangent: self.f32()?,
            out_tangent: self.f32()?,
            in_weight: self.f32()?,
            out_weight: self.f32()?,
            value: self.f32()?,
        })
    }
    fn count(&mut self, item: usize) -> Result<usize> {
        let at = self.at;
        let n = self.u32()? as usize;
        if n.saturating_mul(item) > self.data.len() - self.at.min(self.data.len()) {
            return Err(Error::malformed(WHAT, at, format!("{n} keys")));
        }
        Ok(n)
    }
    fn transform_keys(&mut self) -> Result<(Vec<PositionKey>, Vec<RotationKey>)> {
        let n = self.count(36)?;
        let positions = (0..n)
            .map(|_| {
                Ok(PositionKey {
                    time: self.f32()?,
                    position: self.vec3()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let rotations = (0..n)
            .map(|_| {
                Ok(RotationKey {
                    time: self.f32()?,
                    rotation: self.vec4()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((positions, rotations))
    }
    fn curves(&mut self) -> Result<Curves> {
        let at = self.at;
        let counts: Vec<usize> = (0..6)
            .map(|_| self.u32().map(|n| n as usize))
            .collect::<Result<_>>()?;
        let total: usize = counts.iter().fold(0usize, |a, &n| a.saturating_add(n));
        if total.saturating_mul(24) > self.data.len() - self.at.min(self.data.len()) {
            return Err(Error::malformed(WHAT, at, format!("{total} curve keys")));
        }
        let mut curves: Curves = Default::default();
        for (curve, &n) in curves.iter_mut().zip(&counts) {
            *curve = (0..n).map(|_| self.curve_key()).collect::<Result<_>>()?;
        }
        Ok(curves)
    }
}

/// Parses one command; `body` is the command without its 4-byte header.
fn parse_command(code: u16, body: &[u8], at: usize) -> Result<Command> {
    let mut r = Reader { data: body, at: 0 };
    let command = match code {
        code::WAIT if body.len() == 4 => Command::Wait { seconds: r.f32()? },
        code::WAIT_ACTOR if body.len() == 20 => Command::WaitActor {
            actor: r.guid()?,
            condition: r.u32()?,
        },
        code::ACTOR_CREATE if body.len() == 0x4C => Command::ActorCreate {
            actor: r.guid()?,
            name: r.text(32, 0)?,
            flags: [r.u32()?, r.u32()?, r.u32()?],
            position: r.vec3()?,
            reserved: r.u32()?,
        },
        code::ACTOR_ANIM if body.len() == 0x68 => Command::ActorAnim {
            actor: r.guid()?,
            clip: r.text(64, FILL)?,
            speed: r.f32()?,
            params: [r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?],
        },
        code::ACTOR_TRANSFORM => {
            let actor = r.guid()?;
            let (positions, rotations) = r.transform_keys()?;
            Command::ActorTransform {
                actor,
                positions,
                rotations,
            }
        }
        code::CAMERA_CREATE if body.len() == 12 => Command::CameraCreate {
            camera: r.u32()?,
            flags: r.u32()?,
            aspect: r.f32()?,
        },
        code::CAMERA_CURVES => Command::CameraCurves {
            camera: r.u32()?,
            curves: r.curves()?,
        },
        code::CAMERA_FOV => {
            let camera = r.u32()?;
            let n = r.count(24)?;
            Command::CameraFov {
                camera,
                keys: (0..n).map(|_| r.curve_key()).collect::<Result<_>>()?,
            }
        }
        code::SCRIPT if body.len() >= 3 => {
            let kind = r.u8()?;
            let capacity = r.u8()?;
            let text = r.text(capacity as usize, FILL)?;
            Command::Script {
                kind,
                capacity,
                text,
                trailer: r.u8()?,
            }
        }
        code::FADE if body.len() == 24 => Command::Fade {
            mode: r.u32()?,
            seconds: r.f32()?,
            color: r.vec4()?,
        },
        code::VFX_CREATE if body.len() == 0x60 => Command::VfxCreate {
            vfx: r.u32()?,
            resource: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
            name: r.text(64, FILL)?,
            position: r.vec3()?,
        },
        code::VFX_TRANSFORM => {
            let vfx = r.u32()?;
            let (positions, rotations) = r.transform_keys()?;
            Command::VfxTransform {
                vfx,
                positions,
                rotations,
            }
        }
        code::VFX_STATE if body.len() == 8 => Command::VfxState {
            vfx: r.u32()?,
            state: r.u32()?,
        },
        code::VFX_ATTACH if body.len() == 0x40 => Command::VfxAttach {
            vfx: r.u32()?,
            actor: r.guid()?,
            bone: r.text(32, FILL)?,
            offset: r.vec3()?,
        },
        code::VFX_CURVES => Command::VfxCurves {
            vfx: r.u32()?,
            curves: r.curves()?,
        },
        _ => {
            return Ok(Command::Raw {
                code,
                data: body.to_vec(),
            });
        }
    };
    if r.at != body.len() {
        // A known command of another size: keep it as it is.
        let _ = at;
        return Ok(Command::Raw {
            code,
            data: body.to_vec(),
        });
    }
    Ok(command)
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_floats(out: &mut Vec<u8>, v: &[f32]) {
    for &f in v {
        put_f32(out, f);
    }
}

fn put_curve_key(out: &mut Vec<u8>, k: &CurveKey) {
    put_floats(
        out,
        &[
            k.time,
            k.in_tangent,
            k.out_tangent,
            k.in_weight,
            k.out_weight,
            k.value,
        ],
    );
}

fn put_transform_keys(
    out: &mut Vec<u8>,
    positions: &[PositionKey],
    rotations: &[RotationKey],
) -> Result<()> {
    if positions.len() != rotations.len() {
        return Err(Error::InvalidArgument(format!(
            "{} position keys and {} rotation keys: the counts must be equal",
            positions.len(),
            rotations.len()
        )));
    }
    put_u32(out, positions.len() as u32);
    for k in positions {
        put_f32(out, k.time);
        put_floats(out, &k.position);
    }
    for k in rotations {
        put_f32(out, k.time);
        put_floats(out, &k.rotation);
    }
    Ok(())
}

fn put_curves(out: &mut Vec<u8>, curves: &Curves) {
    for c in curves {
        put_u32(out, c.len() as u32);
    }
    for k in curves.iter().flatten() {
        put_curve_key(out, k);
    }
}

/// Text buffer size of a new script command: the record stays a multiple of
/// four bytes (record size = buffer + 7).
fn script_capacity(text_len: usize) -> usize {
    (text_len + 1 + 7).div_ceil(4) * 4 - 7
}

impl Command {
    /// The command with its header.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.code().to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        match self {
            Self::Wait { seconds } => put_f32(&mut out, *seconds),
            Self::WaitActor { actor, condition } => {
                out.extend_from_slice(&actor.0);
                put_u32(&mut out, *condition);
            }
            Self::ActorCreate {
                actor,
                name,
                flags,
                position,
                reserved,
            } => {
                out.extend_from_slice(&actor.0);
                name.write(&mut out, 32, 0)?;
                for &f in flags {
                    put_u32(&mut out, f);
                }
                put_floats(&mut out, position);
                put_u32(&mut out, *reserved);
            }
            Self::ActorAnim {
                actor,
                clip,
                speed,
                params,
            } => {
                out.extend_from_slice(&actor.0);
                clip.write(&mut out, 64, FILL)?;
                put_f32(&mut out, *speed);
                for &p in params {
                    put_u32(&mut out, p);
                }
            }
            Self::ActorTransform {
                actor,
                positions,
                rotations,
            } => {
                out.extend_from_slice(&actor.0);
                put_transform_keys(&mut out, positions, rotations)?;
            }
            Self::CameraCreate {
                camera,
                flags,
                aspect,
            } => {
                put_u32(&mut out, *camera);
                put_u32(&mut out, *flags);
                put_f32(&mut out, *aspect);
            }
            Self::CameraCurves { camera, curves } => {
                put_u32(&mut out, *camera);
                put_curves(&mut out, curves);
            }
            Self::CameraFov { camera, keys } => {
                put_u32(&mut out, *camera);
                put_u32(&mut out, keys.len() as u32);
                for k in keys {
                    put_curve_key(&mut out, k);
                }
            }
            Self::Script {
                kind,
                capacity,
                text,
                trailer,
            } => {
                let len = text.bytes()?.len();
                let fits = len < *capacity as usize
                    && text
                        .slack
                        .as_ref()
                        .is_some_and(|s| s.len() == *capacity as usize - len - 1)
                    || len < *capacity as usize && script_capacity(len) == *capacity as usize;
                let capacity = if fits {
                    *capacity as usize
                } else {
                    script_capacity(len)
                };
                if capacity > 255 {
                    return Err(Error::InvalidArgument(format!(
                        "`{}` is longer than 247 bytes",
                        text.text
                    )));
                }
                out.push(*kind);
                out.push(capacity as u8);
                text.write(&mut out, capacity, FILL)?;
                out.push(if fits { *trailer } else { 0 });
            }
            Self::Fade {
                mode,
                seconds,
                color,
            } => {
                put_u32(&mut out, *mode);
                put_f32(&mut out, *seconds);
                put_floats(&mut out, color);
            }
            Self::VfxCreate {
                vfx,
                resource,
                name,
                position,
            } => {
                put_u32(&mut out, *vfx);
                for &w in resource {
                    put_u32(&mut out, w);
                }
                name.write(&mut out, 64, FILL)?;
                put_floats(&mut out, position);
            }
            Self::VfxTransform {
                vfx,
                positions,
                rotations,
            } => {
                put_u32(&mut out, *vfx);
                put_transform_keys(&mut out, positions, rotations)?;
            }
            Self::VfxState { vfx, state } => {
                put_u32(&mut out, *vfx);
                put_u32(&mut out, *state);
            }
            Self::VfxAttach {
                vfx,
                actor,
                bone,
                offset,
            } => {
                put_u32(&mut out, *vfx);
                out.extend_from_slice(&actor.0);
                bone.write(&mut out, 32, FILL)?;
                put_floats(&mut out, offset);
            }
            Self::VfxCurves { vfx, curves } => {
                put_u32(&mut out, *vfx);
                put_curves(&mut out, curves);
            }
            Self::Raw { data, .. } => out.extend_from_slice(data),
        }
        let size = u16::try_from(out.len()).map_err(|_| {
            Error::InvalidArgument(format!("command {:04x} is over 64 KiB", self.code()))
        })?;
        out[2..4].copy_from_slice(&size.to_le_bytes());
        Ok(out)
    }
}

impl SeqBlock {
    fn parse(data: &[u8], at: usize) -> Result<Self> {
        let mut r = Reader { data, at };
        if r.bytes(4)? != b"seqb" {
            return Err(Error::malformed(WHAT, at, "no `seqb` block"));
        }
        let version = r.u32()?;
        let hash = r.u32()?;
        let mut transform = [0.0; 9];
        for t in &mut transform {
            *t = r.f32()?;
        }
        let name = r.text(NAME_LEN, 0)?;
        let size = r.u32()? as usize;
        let duration = r.f32()?;
        let first = r.u32()? as usize;
        let end = at
            .checked_add(size)
            .filter(|&e| e <= data.len())
            .ok_or_else(|| Error::malformed(WHAT, at + 0xB0, format!("block size {size}")))?;
        if first != BLOCK_HEADER {
            return Err(Error::malformed(
                WHAT,
                at + 0xB8,
                format!("commands at {first:#x}"),
            ));
        }
        let mut p = at + first;
        let mut commands = Vec::new();
        loop {
            if p + 4 > end {
                return Err(Error::malformed(WHAT, p, "command list without an end"));
            }
            let code = u16::from_le_bytes([data[p], data[p + 1]]);
            let len = u16::from_le_bytes([data[p + 2], data[p + 3]]) as usize;
            if len < 4 || p + len > end {
                return Err(Error::malformed(WHAT, p, format!("command size {len}")));
            }
            if code == code::END {
                if len != 4 || p + len != end {
                    return Err(Error::malformed(WHAT, p, "data after the end command"));
                }
                break;
            }
            commands.push(parse_command(code, &data[p + 4..p + len], p)?);
            p += len;
        }
        Ok(Self {
            version,
            hash,
            transform,
            name,
            duration,
            commands,
        })
    }

    fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        let start = out.len();
        out.extend_from_slice(b"seqb");
        put_u32(out, self.version);
        put_u32(out, self.hash);
        put_floats(out, &self.transform);
        self.name.write(out, NAME_LEN, 0)?;
        put_u32(out, 0); // size, below
        put_f32(out, self.duration);
        put_u32(out, BLOCK_HEADER as u32);
        out.extend_from_slice(&[FILL; 4]);
        for command in &self.commands {
            if matches!(command, Command::Raw { code, .. } if *code == code::END) {
                return Err(Error::InvalidArgument("the end command is implicit".into()));
            }
            out.extend_from_slice(&command.to_bytes()?);
        }
        out.extend_from_slice(&code::END.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        let size = (out.len() - start) as u32;
        out[start + 0xB0..start + 0xB4].copy_from_slice(&size.to_le_bytes());
        Ok(())
    }

    /// Start time (seconds from the start of the block) of every command:
    /// the sum of the waits before it.
    pub fn times(&self) -> Vec<f32> {
        let mut t = 0.0f32;
        self.commands
            .iter()
            .map(|c| {
                let at = t;
                if let Command::Wait { seconds } = c {
                    t += seconds;
                }
                at
            })
            .collect()
    }

    /// Sum of all waits: when the command list ends.
    pub fn wait_total(&self) -> f32 {
        self.commands
            .iter()
            .map(|c| match c {
                Command::Wait { seconds } => *seconds,
                _ => 0.0,
            })
            .sum()
    }

    /// Moves command `index` (not a wait) to `time` seconds from the start of
    /// the block. The waits around its old and new places are merged and
    /// split so that every other command keeps its time.
    pub fn move_command(&mut self, index: usize, time: f32) -> Result<()> {
        if matches!(self.commands.get(index), None | Some(Command::Wait { .. })) {
            return Err(Error::InvalidArgument(format!(
                "no movable command {index}"
            )));
        }
        let command = self.commands.remove(index);
        self.merge_waits_at(index);
        self.insert_at(time, command);
        Ok(())
    }

    /// Removes command `index`; waits keep the times of the others.
    pub fn remove_command(&mut self, index: usize) -> Option<Command> {
        if index >= self.commands.len() {
            return None;
        }
        let command = self.commands.remove(index);
        if !matches!(command, Command::Wait { .. }) {
            self.merge_waits_at(index);
        }
        Some(command)
    }

    /// Merges the waits on both sides of position `index`, if both are waits.
    fn merge_waits_at(&mut self, index: usize) {
        if index == 0 || index >= self.commands.len() {
            return;
        }
        if let (Command::Wait { seconds: a }, Command::Wait { seconds: b }) =
            (&self.commands[index - 1], &self.commands[index])
        {
            let sum = a + b;
            self.commands[index - 1] = Command::Wait { seconds: sum };
            self.commands.remove(index);
        }
    }

    /// Inserts a command (not a wait) to run `time` seconds after the start of
    /// the block, after the commands that already run at that time.
    pub fn insert_at(&mut self, time: f32, command: Command) {
        let time = time.max(0.0);
        let mut t = 0.0f32;
        for i in 0..self.commands.len() {
            let Command::Wait { seconds } = self.commands[i] else {
                continue;
            };
            if t + seconds > time {
                let before = time - t;
                if before <= 0.0 {
                    self.commands.insert(i, command);
                } else {
                    self.commands[i] = Command::Wait {
                        seconds: seconds - before,
                    };
                    self.commands.insert(i, command);
                    self.commands.insert(i, Command::Wait { seconds: before });
                }
                return;
            }
            t += seconds;
        }
        if time > t {
            self.commands.push(Command::Wait { seconds: time - t });
        }
        self.commands.push(command);
    }
}

impl SeqMaster {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader { data, at: 0 };
        if r.bytes(4)? != b"seqm" {
            return Err(Error::BadMagic {
                what: WHAT,
                found: u32::from_be_bytes(data[..4].try_into().expect("4 bytes")),
            });
        }
        let path = r.text(PATH_LEN, 0)?;
        let size = r.u32()? as usize;
        if size != data.len() {
            return Err(Error::malformed(
                WHAT,
                0x108,
                format!("size {size}, data {}", data.len()),
            ));
        }
        let count = r.u32()? as usize;
        let table = r.u32()? as usize;
        if table != BLOCK_TABLE || count > BLOCK_SLOTS {
            return Err(Error::malformed(
                WHAT,
                0x10C,
                format!("{count} blocks, table at {table:#x}"),
            ));
        }
        r.at = table;
        let offsets: Vec<usize> = (0..count)
            .map(|_| r.u32().map(|o| o as usize))
            .collect::<Result<_>>()?;
        let blocks = offsets
            .iter()
            .map(|&o| SeqBlock::parse(data, o))
            .collect::<Result<_>>()?;
        Ok(Self { path, blocks })
    }

    /// The resource as the game stores it.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.blocks.len() > BLOCK_SLOTS {
            return Err(Error::InvalidArgument(format!(
                "{} blocks",
                self.blocks.len()
            )));
        }
        let mut out = Vec::with_capacity(4096);
        out.extend_from_slice(b"seqm");
        self.path.write(&mut out, PATH_LEN, 0)?;
        put_u32(&mut out, 0); // size, below
        put_u32(&mut out, self.blocks.len() as u32);
        put_u32(&mut out, BLOCK_TABLE as u32);
        out.resize(HEADER_SIZE, FILL);
        for (i, block) in self.blocks.iter().enumerate() {
            let at = out.len() as u32;
            out[BLOCK_TABLE + 4 * i..BLOCK_TABLE + 4 * i + 4].copy_from_slice(&at.to_le_bytes());
            block.write(&mut out)?;
        }
        let size = out.len() as u32;
        out[0x108..0x10C].copy_from_slice(&size.to_le_bytes());
        Ok(out)
    }

    /// The first block (every shot of the game has exactly one).
    pub fn block(&self) -> Option<&SeqBlock> {
        self.blocks.first()
    }

    /// The first block, mutable.
    pub fn block_mut(&mut self) -> Option<&mut SeqBlock> {
        self.blocks.first_mut()
    }

    /// File name of the source XML without the extension (`igc01_s01`); the
    /// shot's stream is named after it.
    pub fn source_name(&self) -> &str {
        let name = self.path.text.rsplit(['\\', '/']).next().unwrap_or("");
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    /// The cut-scene folder of the source XML (`igc01` in
    /// `x:\assets\igcs\igc01\source\igc01_s01.xml`).
    pub fn scene_name(&self) -> Option<&str> {
        let parts: Vec<&str> = self.path.text.split(['\\', '/']).collect();
        let source = parts
            .iter()
            .rposition(|p| p.eq_ignore_ascii_case("source"))?;
        parts.get(source.checked_sub(1)?).copied()
    }

    /// The resource id the game gives a shot compiled from `path`.
    pub fn id_for_path(path: &str) -> u32 {
        sdbm(path.to_ascii_lowercase().as_bytes())
    }
}

/// Camera transform at the given curve values: position and rotation
/// `Rz · Ry · Rx` from degrees, as a column-major 4×4 matrix (camera to world).
pub fn camera_matrix(values: [f32; 6]) -> [f32; 16] {
    let [x, y, z, rx, ry, rz] = values;
    let (sx, cx) = rx.to_radians().sin_cos();
    let (sy, cy) = ry.to_radians().sin_cos();
    let (sz, cz) = rz.to_radians().sin_cos();
    // R = Rz * Ry * Rx (column vectors).
    let r = [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ];
    [
        r[0][0], r[1][0], r[2][0], 0.0, //
        r[0][1], r[1][1], r[2][1], 0.0, //
        r[0][2], r[1][2], r[2][2], 0.0, //
        x, y, z, 1.0,
    ]
}

/// Rotation `Rz · Ry · Rx` from degrees as a quaternion `(x, y, z, w)`.
pub fn euler_quaternion(rx: f32, ry: f32, rz: f32) -> [f32; 4] {
    let half = |d: f32| (d.to_radians() * 0.5).sin_cos();
    let (sx, cx) = half(rx);
    let (sy, cy) = half(ry);
    let (sz, cz) = half(rz);
    // q = qz * qy * qx
    [
        sx * cy * cz - cx * sy * sz,
        cx * sy * cz + sx * cy * sz,
        cx * cy * sz - sx * sy * cz,
        cx * cy * cz + sx * sy * sz,
    ]
}

/// The six camera curves evaluated at `t`.
pub fn eval_curves(curves: &Curves, t: f32) -> [f32; 6] {
    std::array::from_fn(|i| eval_curve(&curves[i], t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(n: u8) -> Guid {
        let mut g = [n; 16];
        g[8..].copy_from_slice(b"DOCTORTK");
        Guid(g)
    }

    fn sample() -> SeqMaster {
        let actor = guid(1);
        let wait = |seconds| Command::Wait { seconds };
        let commands = vec![
            Command::ActorCreate {
                actor,
                name: FixedText::new("igc99_hero"),
                flags: [0; 3],
                position: [1.0, 2.0, 3.0],
                reserved: 0,
            },
            Command::CameraCreate {
                camera: 0,
                flags: 0,
                aspect: 1.778,
            },
            wait(0.0),
            Command::ActorTransform {
                actor,
                positions: vec![PositionKey {
                    time: 0.0,
                    position: [1.0, 2.0, 3.0],
                }],
                rotations: vec![RotationKey {
                    time: 0.0,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }],
            },
            wait(0.0),
            Command::ActorAnim {
                actor,
                clip: FixedText::new("igc99_hero_igc99_s01"),
                speed: 1.0,
                params: [0, 120, 8, 1, 0],
            },
            wait(0.0),
            Command::CameraCurves {
                camera: 0,
                curves: std::array::from_fn(|i| {
                    vec![
                        CurveKey::flat(0.0, i as f32),
                        CurveKey::flat(2.0, i as f32 + 1.0),
                    ]
                }),
            },
            wait(0.0),
            Command::CameraFov {
                camera: 0,
                keys: vec![CurveKey::flat(0.0, 50.0)],
            },
            wait(0.5),
            Command::Script {
                kind: 1,
                capacity: 13,
                text: FixedText::new("music_igc_99"),
                trailer: 0,
            },
            wait(0.25),
            Command::Fade {
                mode: 1,
                seconds: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
            },
            wait(1.25),
            Command::Raw {
                code: 0x0701,
                data: vec![1, 2, 3, 4],
            },
        ];
        SeqMaster {
            path: FixedText::new("x:\\assets\\igcs\\igc99\\source\\igc99_s01.xml"),
            blocks: vec![SeqBlock {
                version: 4,
                hash: 0x1234_5678,
                transform: [-0.0, -0.0, -0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                name: FixedText::new("trSeq_igc99_s01"),
                duration: 2.0,
                commands,
            }],
        }
    }

    #[test]
    fn writes_and_reads_back() {
        let seq = sample();
        let bytes = seq.to_bytes().unwrap();
        assert_eq!(&bytes[..4], b"seqm");
        assert_eq!(
            u32::from_le_bytes(bytes[0x108..0x10C].try_into().unwrap()) as usize,
            bytes.len()
        );
        assert_eq!(&bytes[0x520..0x524], b"seqb");
        let back = SeqMaster::parse(&bytes).unwrap();
        assert_eq!(back, seq);
        assert_eq!(back.to_bytes().unwrap(), bytes);
        assert_eq!(back.source_name(), "igc99_s01");
        assert_eq!(back.scene_name(), Some("igc99"));
        // JSON round trip keeps every bit (negative zeros included).
        let json = serde_json::to_string(&back).unwrap();
        let from_json: SeqMaster = serde_json::from_str(&json).unwrap();
        assert_eq!(from_json.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn script_commands_keep_their_buffer() {
        // "restoreShadowRanges" in a 21-byte buffer with garbage after the zero.
        let mut body = vec![2u8, 21];
        body.extend_from_slice(b"restoreShadowRanges\0\xbb");
        body.push(3);
        let command = parse_command(code::SCRIPT, &body, 0).unwrap();
        let bytes = command.to_bytes().unwrap();
        assert_eq!(bytes.len(), 28);
        assert_eq!(&bytes[4..], &body[..]);
        // A new text gets a buffer that keeps the record a multiple of 4.
        let Command::Script { kind, trailer, .. } = command else {
            panic!("not a script command")
        };
        let longer = Command::Script {
            kind,
            capacity: 21,
            text: FixedText::new("setShadowRanges 2.0 28.0 75.0 0.0"),
            trailer,
        };
        let bytes = longer.to_bytes().unwrap();
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(bytes[5] as usize + 7, bytes.len());
    }

    #[test]
    fn rejects_broken_input() {
        let bytes = sample().to_bytes().unwrap();
        assert!(SeqMaster::parse(&bytes[..bytes.len() - 4]).is_err());
        let mut b = bytes.clone();
        b[0] = b'x';
        assert!(SeqMaster::parse(&b).is_err());
        let mut b = bytes.clone();
        let end = b.len() - 4;
        b[end..end + 2].copy_from_slice(&0x0101u16.to_le_bytes());
        assert!(SeqMaster::parse(&b).is_err());
        assert!(SeqMaster::parse(b"seqm").is_err());
    }

    #[test]
    fn curves_follow_the_tangents() {
        // A straight line: tangent angle of the slope per frame.
        let slope_per_second = 6.0f32;
        let angle = (slope_per_second / CURVE_FRAME_RATE).atan();
        let key = |time, value| CurveKey {
            time,
            in_tangent: angle,
            out_tangent: angle,
            in_weight: 1.0,
            out_weight: 1.0,
            value,
        };
        let keys = [key(0.0, 10.0), key(2.0, 22.0)];
        for t in [0.0, 0.5, 1.0, 1.7, 2.0] {
            assert!(
                (eval_curve(&keys, t) - (10.0 + 6.0 * t)).abs() < 1e-4,
                "t = {t}"
            );
        }
        assert_eq!(eval_curve(&keys, -1.0), 10.0);
        assert_eq!(eval_curve(&keys, 5.0), 22.0);
        assert_eq!(eval_curve(&[], 1.0), 0.0);
        // Flat tangents ease in and out.
        let flat = [CurveKey::flat(0.0, 0.0), CurveKey::flat(1.0, 1.0)];
        assert!((eval_curve(&flat, 0.5) - 0.5).abs() < 1e-6);
        assert!(eval_curve(&flat, 0.1) < 0.1);
    }

    #[test]
    fn camera_rotation_order() {
        // Pitch down by 50 degrees after a half turn: the camera looks along +Z and down.
        let m = camera_matrix([0.0, 0.0, 0.0, -50.0, 180.0, 0.0]);
        let forward = [-m[8], -m[9], -m[10]];
        assert!(forward[2] > 0.6 && forward[1] < -0.7, "{forward:?}");
        // The quaternion matches the matrix.
        let q = euler_quaternion(30.0, -70.0, 20.0);
        let m = camera_matrix([0.0, 0.0, 0.0, 30.0, -70.0, 20.0]);
        let [x, y, z, w] = q;
        let col0 = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        for k in 0..3 {
            assert!((col0[k] - m[k]).abs() < 1e-5);
        }
    }

    #[test]
    fn moving_a_command_keeps_the_others_in_place() {
        let mut seq = sample();
        let block = &mut seq.blocks[0];
        let before = block.times();
        let others: Vec<(String, f32)> = block
            .commands
            .iter()
            .zip(&before)
            .filter(|(c, _)| !matches!(c, Command::Wait { .. } | Command::Script { .. }))
            .map(|(c, t)| (c.label(), *t))
            .collect();
        let script = block
            .commands
            .iter()
            .position(|c| matches!(c, Command::Script { .. }))
            .unwrap();
        block.move_command(script, 1.6).unwrap();
        let times = block.times();
        let script = block
            .commands
            .iter()
            .position(|c| matches!(c, Command::Script { .. }))
            .unwrap();
        assert!((times[script] - 1.6).abs() < 1e-6);
        let after: Vec<(String, f32)> = block
            .commands
            .iter()
            .zip(&times)
            .filter(|(c, _)| !matches!(c, Command::Wait { .. } | Command::Script { .. }))
            .map(|(c, t)| (c.label(), *t))
            .collect();
        assert_eq!(others.len(), after.len());
        for (a, b) in others.iter().zip(&after) {
            assert_eq!(a.0, b.0);
            assert!((a.1 - b.1).abs() < 1e-6);
        }
        assert!((block.wait_total() - 2.0).abs() < 1e-6);
        // Past the end: the list grows.
        block.move_command(script, 3.0).unwrap();
        assert!((block.wait_total() - 3.0).abs() < 1e-6);
        assert!(
            block.move_command(2, 1.0).is_err()
                || !matches!(block.commands[2], Command::Wait { .. })
        );
        // Still a valid resource.
        let bytes = seq.to_bytes().unwrap();
        assert_eq!(SeqMaster::parse(&bytes).unwrap(), seq);
    }
}
