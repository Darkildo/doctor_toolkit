//! `doctor_cheats.asi`: a developer menu in the game for the player of The
//! Godfather II (PC 1.0.789.0) — a native mod on `doctor_platform.asi`
//! (stage 119 of `PLAN-cheats.md`).
//!
//! The key `~` (DirectInput `DIK_GRAVE`, any layout; `key` in
//! `scripts/doctor_cheats.cfg`) or Back + RB on a gamepad opens a menu drawn
//! over the frame in the colours of the game's menus, also on pause. While
//! it is open the game sees no input (the platform's input gate); arrows or
//! the D-pad choose, Enter or A does, Esc or B goes back and closes. The
//! texts are English or Russian, after the game's language.
//!
//! What the platform gives and the mod uses: the frame tick (the menu and
//! its actions run on the main thread), the overlay, the input gate with
//! the reserved key; the drawing is `doctor_overlay`. The items of later
//! stages are shown dimmed, "soon".

#![cfg_attr(not(all(windows, target_arch = "x86")), allow(dead_code))]

pub mod config;
pub mod menu;
pub mod text;
pub mod view;

#[cfg(all(windows, target_arch = "x86"))]
mod game;
#[cfg(all(windows, target_arch = "x86"))]
mod hooks;
#[cfg(all(windows, target_arch = "x86"))]
mod module;

/// Version written to the platform's log.
pub const VERSION: &str = concat!("doctor_cheats ", env!("CARGO_PKG_VERSION"));
