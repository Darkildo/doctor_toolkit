//! The Direct3D 9 calls of the painter, through the vtables of the game's
//! device and of the atlas texture (slots as in doctor_render's `d3d.rs`).

use std::ffi::c_void;

pub type Object = *mut c_void;

pub const D3DFMT_A8R8G8B8: u32 = 21;
pub const D3DPOOL_MANAGED: u32 = 1;
pub const D3DPT_TRIANGLELIST: u32 = 4;
/// `D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1`.
pub const FVF: u32 = 0x004 | 0x040 | 0x100;

pub const RS_ZENABLE: u32 = 7;
pub const RS_FILLMODE: u32 = 8;
pub const RS_ZWRITEENABLE: u32 = 14;
pub const RS_ALPHATESTENABLE: u32 = 15;
pub const RS_SRCBLEND: u32 = 19;
pub const RS_DESTBLEND: u32 = 20;
pub const RS_CULLMODE: u32 = 22;
pub const RS_ALPHABLENDENABLE: u32 = 27;
pub const RS_FOGENABLE: u32 = 28;
pub const RS_STENCILENABLE: u32 = 52;
pub const RS_LIGHTING: u32 = 137;
pub const RS_CLIPPLANEENABLE: u32 = 152;
pub const RS_COLORWRITEENABLE: u32 = 168;
pub const RS_BLENDOP: u32 = 171;
pub const RS_SCISSORTESTENABLE: u32 = 174;
pub const RS_SRGBWRITEENABLE: u32 = 194;
pub const RS_SEPARATEALPHABLENDENABLE: u32 = 206;

pub const TSS_COLOROP: u32 = 1;
pub const TSS_COLORARG1: u32 = 2;
pub const TSS_COLORARG2: u32 = 3;
pub const TSS_ALPHAOP: u32 = 4;
pub const TSS_ALPHAARG1: u32 = 5;
pub const TSS_ALPHAARG2: u32 = 6;
pub const TSS_TEXCOORDINDEX: u32 = 11;
pub const TSS_TEXTURETRANSFORMFLAGS: u32 = 24;
pub const TOP_DISABLE: u32 = 1;
pub const TOP_MODULATE: u32 = 4;
pub const TA_DIFFUSE: u32 = 0;
pub const TA_TEXTURE: u32 = 2;

pub const SAMP_ADDRESSU: u32 = 1;
pub const SAMP_ADDRESSV: u32 = 2;
pub const SAMP_MAGFILTER: u32 = 5;
pub const SAMP_MINFILTER: u32 = 6;
pub const SAMP_MIPFILTER: u32 = 7;
pub const SAMP_SRGBTEXTURE: u32 = 11;
pub const TEXF_NONE: u32 = 0;
pub const TEXF_LINEAR: u32 = 2;
pub const TADDRESS_CLAMP: u32 = 3;

/// `D3DLOCKED_RECT`.
#[repr(C)]
pub struct LockedRect {
    pub pitch: i32,
    pub bits: *mut u8,
}

/// `RECT`.
#[repr(C)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The function at slot `index` of the vtable of `object`.
///
/// # Safety
/// `object` must be a live COM object with at least `index + 1` slots.
unsafe fn slot(object: Object, index: usize) -> usize {
    // SAFETY: the caller's promise.
    unsafe { *(*(object as *const *const usize)).add(index) }
}

macro_rules! com {
    ($name:ident, $index:expr, ($($arg:ident: $ty:ty),*)) => {
        /// # Safety
        /// `object` must be a live object of the right interface.
        #[allow(clippy::too_many_arguments)]
        pub unsafe fn $name(object: Object $(, $arg: $ty)*) -> i32 {
            // SAFETY: the caller's promise; stdcall like every COM method.
            unsafe {
                let f: extern "system" fn(Object $(, $ty)*) -> i32 =
                    std::mem::transmute(slot(object, $index));
                f(object $(, $arg)*)
            }
        }
    };
}

com!(release, 2, ());
// IDirect3DDevice9
com!(create_texture, 23, (w: u32, h: u32, levels: u32, usage: u32, format: u32, pool: u32, out: *mut Object, shared: *mut c_void));
com!(set_render_state, 57, (state: u32, value: u32));
com!(set_texture, 65, (stage: u32, texture: Object));
com!(set_texture_stage_state, 67, (stage: u32, kind: u32, value: u32));
com!(set_sampler_state, 69, (sampler: u32, kind: u32, value: u32));
com!(set_scissor_rect, 75, (rect: *const Rect));
com!(draw_primitive_up, 83, (kind: u32, count: u32, data: *const c_void, stride: u32));
com!(set_fvf, 89, (fvf: u32));
com!(set_vertex_shader, 92, (shader: Object));
com!(set_pixel_shader, 107, (shader: Object));
// IDirect3DTexture9
com!(lock_rect, 19, (level: u32, out: *mut LockedRect, rect: *const Rect, flags: u32));
com!(unlock_rect, 20, (level: u32));
