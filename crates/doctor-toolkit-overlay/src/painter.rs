//! Rectangles and text over the frame, inside an overlay listener of the
//! platform (`overlay_listen`), with the device the platform gives: its
//! back buffer is the target, the platform has saved the state and puts it
//! back after the listener.

use std::ffi::c_void;

use doctor_sdk::abi::Overlay;

use crate::atlas::{Atlas, Face, WHITE};
use crate::d3d::{self, Object};

/// A colour, `0xAARRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self((a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    /// The same colour with alpha `a`.
    pub const fn alpha(self, a: u8) -> Self {
        Self(self.0 & 0x00FF_FFFF | (a as u32) << 24)
    }
}

/// A vertex of `D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    color: u32,
    u: f32,
    v: f32,
}

/// The fonts and the atlas texture of one mod; keep it for the life of the
/// mod (a `static` behind a mutex) and draw with [`Painter::frame`].
pub struct Painter {
    atlas: Option<Atlas>,
    device: Object,
    texture: Object,
    failed: bool,
    vertices: Vec<Vertex>,
}

// SAFETY: the device and the texture are used only inside overlay
// listeners, one at a time (the platform calls them in turn).
unsafe impl Send for Painter {}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub const fn new() -> Self {
        Self {
            atlas: None,
            device: std::ptr::null_mut(),
            texture: std::ptr::null_mut(),
            failed: false,
            vertices: Vec::new(),
        }
    }

    /// The fonts, built on first use (also outside a frame, to measure).
    pub fn atlas(&mut self) -> Option<&Atlas> {
        if self.atlas.is_none() && !self.failed {
            match Atlas::build() {
                Ok(a) => self.atlas = Some(a),
                Err(_) => self.failed = true,
            }
        }
        self.atlas.as_ref()
    }

    /// Makes the texture of the atlas on `device` (`D3DPOOL_MANAGED`: it
    /// lives through `Reset`).
    ///
    /// # Safety
    /// `device` is the game's device, inside an overlay listener.
    unsafe fn texture(&mut self, device: Object) -> Option<Object> {
        if device != self.device {
            if !self.texture.is_null() && !self.device.is_null() {
                // SAFETY: the texture of the old device.
                unsafe { d3d::release(self.texture) };
            }
            self.texture = std::ptr::null_mut();
            self.device = device;
        }
        if !self.texture.is_null() {
            return Some(self.texture);
        }
        if self.failed {
            return None;
        }
        let atlas = self.atlas()?;
        let (w, h) = (atlas.width, atlas.height);
        let alpha = atlas.alpha.clone();
        let mut texture: Object = std::ptr::null_mut();
        // SAFETY: the caller's promise; the texture is filled while locked.
        unsafe {
            if d3d::create_texture(
                device,
                w,
                h,
                1,
                0,
                d3d::D3DFMT_A8R8G8B8,
                d3d::D3DPOOL_MANAGED,
                &mut texture,
                std::ptr::null_mut(),
            ) < 0
                || texture.is_null()
            {
                self.failed = true;
                return None;
            }
            let mut locked = d3d::LockedRect {
                pitch: 0,
                bits: std::ptr::null_mut(),
            };
            if d3d::lock_rect(texture, 0, &mut locked, std::ptr::null(), 0) < 0
                || locked.bits.is_null()
            {
                d3d::release(texture);
                self.failed = true;
                return None;
            }
            for y in 0..h as usize {
                let row = locked.bits.add(y * locked.pitch as usize) as *mut u32;
                for x in 0..w as usize {
                    let a = alpha[y * w as usize + x] as u32;
                    *row.add(x) = a << 24 | 0x00FF_FFFF;
                }
            }
            d3d::unlock_rect(texture, 0);
        }
        self.texture = texture;
        Some(texture)
    }

    /// Starts drawing in an overlay listener; `None` when the fonts or the
    /// texture could not be made. The frame draws when it is dropped.
    pub fn frame<'a>(&'a mut self, overlay: &Overlay) -> Option<Frame<'a>> {
        let device = overlay.device;
        // SAFETY: the device the platform gives the listener.
        let texture = unsafe { self.texture(device) }?;
        // SAFETY: as above; the platform puts the state back afterwards.
        unsafe { states(device, texture) };
        let scale = (overlay.height as f32 / 720.0).max(0.75);
        self.vertices.clear();
        Some(Frame {
            painter: self,
            device,
            width: overlay.width as f32,
            height: overlay.height as f32,
            scale,
        })
    }
}

/// The state of the device for the painter: fixed function, alpha blending,
/// no depth, the atlas on stage 0.
///
/// # Safety
/// `device` is the game's device, inside an overlay listener.
unsafe fn states(device: Object, texture: Object) {
    use d3d::*;
    // SAFETY: the caller's promise.
    unsafe {
        set_vertex_shader(device, std::ptr::null_mut());
        set_pixel_shader(device, std::ptr::null_mut());
        set_fvf(device, FVF);
        for (state, value) in [
            (RS_ZENABLE, 0),
            (RS_ZWRITEENABLE, 0),
            (RS_FILLMODE, 3),
            (RS_ALPHATESTENABLE, 0),
            (RS_CULLMODE, 1),
            (RS_ALPHABLENDENABLE, 1),
            (RS_SRCBLEND, 5),
            (RS_DESTBLEND, 6),
            (RS_BLENDOP, 1),
            (RS_SEPARATEALPHABLENDENABLE, 0),
            (RS_FOGENABLE, 0),
            (RS_STENCILENABLE, 0),
            (RS_LIGHTING, 0),
            (RS_CLIPPLANEENABLE, 0),
            (RS_COLORWRITEENABLE, 0xF),
            (RS_SCISSORTESTENABLE, 0),
            (RS_SRGBWRITEENABLE, 0),
        ] {
            set_render_state(device, state, value);
        }
        set_texture(device, 0, texture);
        for (kind, value) in [
            (TSS_COLOROP, TOP_MODULATE),
            (TSS_COLORARG1, TA_TEXTURE),
            (TSS_COLORARG2, TA_DIFFUSE),
            (TSS_ALPHAOP, TOP_MODULATE),
            (TSS_ALPHAARG1, TA_TEXTURE),
            (TSS_ALPHAARG2, TA_DIFFUSE),
            (TSS_TEXCOORDINDEX, 0),
            (TSS_TEXTURETRANSFORMFLAGS, 0),
        ] {
            set_texture_stage_state(device, 0, kind, value);
        }
        set_texture_stage_state(device, 1, TSS_COLOROP, TOP_DISABLE);
        set_texture_stage_state(device, 1, TSS_ALPHAOP, TOP_DISABLE);
        for (kind, value) in [
            (SAMP_ADDRESSU, TADDRESS_CLAMP),
            (SAMP_ADDRESSV, TADDRESS_CLAMP),
            (SAMP_MAGFILTER, TEXF_LINEAR),
            (SAMP_MINFILTER, TEXF_LINEAR),
            (SAMP_MIPFILTER, TEXF_NONE),
            (SAMP_SRGBTEXTURE, 0),
        ] {
            set_sampler_state(device, 0, kind, value);
        }
    }
}

/// Drawing of one listener call. Coordinates are pixels of the back
/// buffer; [`Frame::scale`] is the size of a pixel of a 720-line design.
pub struct Frame<'a> {
    painter: &'a mut Painter,
    device: Object,
    pub width: f32,
    pub height: f32,
    /// `height / 720` (at least 0.75): multiply the sizes of a layout made
    /// for 1280×720 by it.
    pub scale: f32,
}

impl Frame<'_> {
    fn quad(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4], color: Color) {
        let (x0, y0, x1, y1) = (x0 - 0.5, y0 - 0.5, x1 - 0.5, y1 - 0.5);
        let v = |x, y, u, v| Vertex {
            x,
            y,
            z: 0.0,
            rhw: 1.0,
            color: color.0,
            u,
            v,
        };
        let [u0, v0, u1, v1] = uv;
        self.painter.vertices.extend_from_slice(&[
            v(x0, y0, u0, v0),
            v(x1, y0, u1, v0),
            v(x0, y1, u0, v1),
            v(x1, y0, u1, v0),
            v(x1, y1, u1, v1),
            v(x0, y1, u0, v1),
        ]);
    }

    /// A solid rectangle; its edges go to whole pixels.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let Some(atlas) = self.painter.atlas.as_ref() else {
            return;
        };
        let u = (WHITE as f32 / 2.0) / atlas.width as f32;
        let v = (WHITE as f32 / 2.0) / atlas.height as f32;
        let (x0, y0) = (x.round(), y.round());
        let (x1, y1) = ((x + w).round(), (y + h).round());
        self.quad(x0, y0, x1, y1, [u, v, u, v], color);
    }

    /// The outline of a rectangle, `t` pixels thick (whole pixels, at
    /// least one, the same on every side).
    pub fn frame_rect(&mut self, x: f32, y: f32, w: f32, h: f32, t: f32, color: Color) {
        let (x, y) = (x.round(), y.round());
        let (w, h) = (w.round(), h.round());
        let t = t.round().max(1.0);
        self.rect(x, y, w, t, color);
        self.rect(x, y + h - t, w, t, color);
        self.rect(x, y + t, t, h - 2.0 * t, color);
        self.rect(x + w - t, y + t, t, h - 2.0 * t, color);
    }

    /// Width of `text` at `px` pixels.
    pub fn measure(&self, face: Face, px: f32, text: &str) -> f32 {
        self.painter
            .atlas
            .as_ref()
            .map_or(0.0, |a| a.font(face).measure(text, px))
    }

    /// Height of a line at `px` pixels.
    pub fn line_height(&self, face: Face, px: f32) -> f32 {
        self.painter
            .atlas
            .as_ref()
            .map_or(px, |a| a.font(face).line_height(px))
    }

    /// One line of `text`, the top of the line at `y`; returns its width.
    pub fn text(&mut self, face: Face, x: f32, y: f32, px: f32, color: Color, text: &str) -> f32 {
        let Some(atlas) = self.painter.atlas.take() else {
            return 0.0;
        };
        let font = atlas.font(face);
        let scale = px / font.raster_px;
        let baseline = (y + font.ascent * scale).round();
        let (aw, ah) = (atlas.width as f32, atlas.height as f32);
        let mut pen = x;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous {
                pen += font.kern(p, c) * scale;
            }
            let g = font.glyph(c);
            if g.width > 0 {
                let x0 = (pen + g.left * scale).round();
                let y0 = baseline - g.top * scale;
                let uv = [
                    g.x as f32 / aw,
                    g.y as f32 / ah,
                    (g.x + g.width) as f32 / aw,
                    (g.y + g.height) as f32 / ah,
                ];
                self.quad(
                    x0,
                    y0,
                    x0 + g.width as f32 * scale,
                    y0 + g.height as f32 * scale,
                    uv,
                    color,
                );
            }
            pen += g.advance * scale;
            previous = Some(c);
        }
        self.painter.atlas = Some(atlas);
        pen - x
    }

    /// Draws only inside `[x, x + w) × [y, y + h)` until [`Frame::unclip`].
    pub fn clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.flush();
        let rect = d3d::Rect {
            left: x as i32,
            top: y as i32,
            right: (x + w) as i32,
            bottom: (y + h) as i32,
        };
        // SAFETY: the device of the listener.
        unsafe {
            d3d::set_scissor_rect(self.device, &rect);
            d3d::set_render_state(self.device, d3d::RS_SCISSORTESTENABLE, 1);
        }
    }

    pub fn unclip(&mut self) {
        self.flush();
        // SAFETY: the device of the listener.
        unsafe { d3d::set_render_state(self.device, d3d::RS_SCISSORTESTENABLE, 0) };
    }

    /// Draws what has been queued.
    pub fn flush(&mut self) {
        let vertices = &self.painter.vertices;
        if vertices.is_empty() {
            return;
        }
        // SAFETY: the device of the listener; the vertices live through the
        // call (DrawPrimitiveUP copies them).
        unsafe {
            d3d::draw_primitive_up(
                self.device,
                d3d::D3DPT_TRIANGLELIST,
                (vertices.len() / 3) as u32,
                vertices.as_ptr() as *const c_void,
                std::mem::size_of::<Vertex>() as u32,
            );
        }
        self.painter.vertices.clear();
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_argb() {
        assert_eq!(Color::rgba(0x11, 0x22, 0x33, 0x44).0, 0x4411_2233);
        assert_eq!(Color::rgb(1, 2, 3).alpha(0x80).0, 0x8001_0203);
        assert_eq!(std::mem::size_of::<Vertex>(), 28);
    }
}
