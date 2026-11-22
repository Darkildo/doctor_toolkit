//! 2D drawing over the frame of The Godfather II (PC 1.0.789.0) for native
//! mods (stage 118).
//!
//! The platform module `doctor_platform.asi` calls an overlay listener of a
//! mod (`overlay_listen`) with the game's device, its back buffer as the
//! target and the state saved. [`Painter`] draws there with the fixed
//! function of Direct3D 9 — no shaders: solid rectangles and text, from one
//! texture (`D3DPOOL_MANAGED`, it lives through `Reset`) that holds the
//! glyphs of two fonts and a white corner.
//!
//! The fonts ([`Face`]) are the ones the mod manager draws the game's menus
//! with: Oswald Medium and Podkova ExtraBold (SIL Open Font License,
//! [`FONT_LICENCES`] — a mod that ships this crate ships the licences). They
//! are in the mod unmodified and rasterised (`fontdue`) the first time the
//! painter draws: Latin, Cyrillic with `Ё`, the typography of menus.
//!
//! ```no_run
//! use std::sync::Mutex;
//! use doctor_overlay::{Color, Face, Painter};
//! use doctor_sdk::abi::Overlay;
//!
//! static PAINTER: Mutex<Painter> = Mutex::new(Painter::new());
//!
//! unsafe extern "C" fn draw(_: *mut core::ffi::c_void, overlay: *const Overlay) {
//!     let Ok(mut painter) = PAINTER.lock() else { return };
//!     // SAFETY: the platform gives a valid description for the call.
//!     let Some(mut f) = painter.frame(unsafe { &*overlay }) else { return };
//!     let s = f.scale;
//!     f.rect(40.0 * s, 40.0 * s, 400.0 * s, 60.0 * s, Color::rgba(20, 16, 12, 220));
//!     f.text(Face::Title, 56.0 * s, 48.0 * s, 32.0 * s, Color::rgb(232, 196, 96), "Меню");
//! }
//! ```

mod atlas;
mod d3d;
mod painter;

pub use atlas::{Atlas, FONT_LICENCES, Face, Font, Glyph, charset};
pub use painter::{Color, Frame, Painter};
