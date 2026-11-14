//! The field of view of the player's camera (`renderer.md` §5).
//!
//! Every frame the camera manager (`0x004252E0`) gives the view it draws
//! with the angle of its camera (horizontal, already widened for the
//! screen's aspect: Hor+) through `SetFov` (`0x00422FB0`) at `0x00425FE4`.
//! The module stands in that call and widens the angle on the tangent of
//! its half, so the zoom of aiming keeps its ratio. The sky, reflections
//! and the other views call `SetFov` from elsewhere and keep their angles.
//! The trace on the bench found no other call that changes the picture in
//! play: the methods of the player's camera at `0x009BD990` that
//! `renderer.md` §5 names are not called on foot.
//!
//! `fov_trace=1` (a test aid) also stands in the seven other calls of
//! `SetFov` and logs how often each place calls it and with what angle.

use std::sync::atomic::{AtomicU32, Ordering};

use doctor_toolkit_live::render::{RenderConfig, addr, fov_factor, scale_angle};

use crate::win;

/// The factor on the tangent, as `f32` bits.
static FACTOR: AtomicU32 = AtomicU32::new(0);

fn call_set_fov(view: u32, angle: f32, aspect: f32) {
    // SAFETY: the game's SetFov, thiscall with two arguments on the stack
    // (checked bytes).
    let original: extern "thiscall" fn(u32, f32, f32) =
        unsafe { std::mem::transmute(addr::SET_FOV as usize) };
    original(view, angle, aspect);
}

fn widen(angle: f32) -> f32 {
    let k = f32::from_bits(FACTOR.load(Ordering::Relaxed));
    if k > 0.0 && angle.is_finite() && angle > 0.0 {
        scale_angle(angle, k)
    } else {
        angle
    }
}

/// Places of the trace: the camera manager first (widened), then the
/// other calls.
const TRACED: [u32; 11] = [
    addr::FOV_CALL,
    0x009B_DA38,
    0x009B_DE5D,
    0x009B_E677,
    0x009B_EB79,
    0x0041_3517,
    0x0041_3698,
    0x0041_374B,
    0x0044_CD4F,
    0x0094_0DD8,
    0x0094_639E,
];
static COUNTS: [AtomicU32; 11] = [const { AtomicU32::new(0) }; 11];
static LAST: [AtomicU32; 11] = [const { AtomicU32::new(0) }; 11];

extern "thiscall" fn set_fov<const I: usize>(view: u32, angle: f32, aspect: f32) {
    COUNTS[I].fetch_add(1, Ordering::Relaxed);
    LAST[I].store(angle.to_bits(), Ordering::Relaxed);
    let angle = if I == 0 { widen(angle) } else { angle };
    call_set_fov(view, angle, aspect);
}

const HOOKS: [extern "thiscall" fn(u32, f32, f32); 11] = [
    set_fov::<0>,
    set_fov::<1>,
    set_fov::<2>,
    set_fov::<3>,
    set_fov::<4>,
    set_fov::<5>,
    set_fov::<6>,
    set_fov::<7>,
    set_fov::<8>,
    set_fov::<9>,
    set_fov::<10>,
];

/// One line: calls of each traced place and its last angle in degrees.
pub fn trace_line() -> String {
    let parts: Vec<String> = TRACED
        .iter()
        .enumerate()
        .filter(|(i, _)| COUNTS[*i].load(Ordering::Relaxed) > 0)
        .map(|(i, at)| {
            format!(
                "{at:#010x} x{} ({:.1}°)",
                COUNTS[i].load(Ordering::Relaxed),
                f32::from_bits(LAST[i].load(Ordering::Relaxed)).to_degrees()
            )
        })
        .collect();
    format!("fov trace: {}", parts.join(", "))
}

pub fn install(config: &RenderConfig, done: &mut Vec<String>) {
    if let Some(k) = fov_factor(config.fov) {
        FACTOR.store(k.to_bits(), Ordering::Relaxed);
    }
    let sites = if config.fov_trace {
        TRACED.len()
    } else if config.fov > 0 {
        1
    } else {
        return;
    };
    let mut n = 0;
    for (i, &call) in TRACED.iter().enumerate().take(sites) {
        if i > 0 {
            // Places outside the checked table (trace only): a call of
            // SetFov, or left alone.
            // SAFETY: inside the image of the game (code).
            let ok = unsafe {
                win::read::<u8>(call) == 0xE8
                    && call
                        .wrapping_add(5)
                        .wrapping_add(win::read::<u32>(call + 1))
                        == addr::SET_FOV
            };
            if !ok {
                continue;
            }
        }
        // SAFETY: checked call instructions of SetFov.
        if unsafe { win::redirect_call(call, HOOKS[i] as usize) } {
            n += 1;
        }
    }
    if config.fov > 0 {
        done.push(format!(
            "field of view {}° at 16:9 (tangent x{:.3}) in the view of the camera manager (0x00425FE4)",
            config.fov,
            f32::from_bits(FACTOR.load(Ordering::Relaxed))
        ));
    }
    if config.fov_trace {
        done.push(format!("test: {n} calls of SetFov traced"));
    }
}
