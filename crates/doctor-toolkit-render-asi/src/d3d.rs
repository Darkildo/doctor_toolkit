//! The few Direct3D 9 calls of the post-effects, through the vtables (the
//! module links no `d3d9.lib`: the device is the game's).

use std::ffi::c_void;

pub type Object = *mut c_void;

pub const D3DUSAGE_RENDERTARGET: u32 = 1;
pub const D3DFMT_A8R8G8B8: u32 = 21;
pub const D3DPOOL_DEFAULT: u32 = 0;
pub const D3DSBT_ALL: u32 = 1;
pub const D3DPT_TRIANGLESTRIP: u32 = 5;

pub const RS_ZENABLE: u32 = 7;
pub const RS_FILLMODE: u32 = 8;
pub const RS_ZWRITEENABLE: u32 = 14;
pub const RS_ALPHATESTENABLE: u32 = 15;
pub const RS_CULLMODE: u32 = 22;
pub const RS_ALPHABLENDENABLE: u32 = 27;
pub const RS_FOGENABLE: u32 = 28;
pub const RS_STENCILENABLE: u32 = 52;
pub const RS_CLIPPLANEENABLE: u32 = 152;
pub const RS_COLORWRITEENABLE: u32 = 168;
pub const RS_SCISSORTESTENABLE: u32 = 174;
pub const RS_SRGBWRITEENABLE: u32 = 194;
pub const RS_SEPARATEALPHABLENDENABLE: u32 = 206;

pub const SAMP_ADDRESSU: u32 = 1;
pub const SAMP_ADDRESSV: u32 = 2;
pub const SAMP_MAGFILTER: u32 = 5;
pub const SAMP_MINFILTER: u32 = 6;
pub const SAMP_MIPFILTER: u32 = 7;
pub const SAMP_MIPMAPLODBIAS: u32 = 8;
pub const SAMP_MAXMIPLEVEL: u32 = 9;
pub const SAMP_SRGBTEXTURE: u32 = 11;
pub const TEXF_NONE: u32 = 0;
pub const TEXF_LINEAR: u32 = 2;
pub const TADDRESS_CLAMP: u32 = 3;

/// `D3DVERTEXELEMENT9`.
#[repr(C)]
pub struct VertexElement {
    pub stream: u16,
    pub offset: u16,
    pub kind: u8,
    pub method: u8,
    pub usage: u8,
    pub usage_index: u8,
}

/// `D3DVIEWPORT9`.
#[repr(C)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub min_z: f32,
    pub max_z: f32,
}

/// `D3DSURFACE_DESC`: width and height at 24 and 28.
#[repr(C)]
#[derive(Default)]
pub struct SurfaceDesc {
    pub format: u32,
    pub kind: u32,
    pub usage: u32,
    pub pool: u32,
    pub multisample: u32,
    pub multisample_quality: u32,
    pub width: u32,
    pub height: u32,
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
    ($(#[$m:meta])* $name:ident, $index:expr, ($($arg:ident: $ty:ty),*)) => {
        $(#[$m])*
        /// # Safety
        /// `object` must be a live object of the right interface.
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
com!(get_back_buffer, 18, (swap: u32, index: u32, kind: u32, out: *mut Object));
com!(create_texture, 23, (w: u32, h: u32, levels: u32, usage: u32, format: u32, pool: u32, out: *mut Object, shared: *mut c_void));
com!(set_render_target, 37, (index: u32, surface: Object));
com!(get_render_target, 38, (index: u32, out: *mut Object));
com!(set_depth_stencil, 39, (surface: Object));
com!(get_depth_stencil, 40, (out: *mut Object));
com!(begin_scene, 41, ());
com!(end_scene, 42, ());
com!(set_viewport, 47, (viewport: *const Viewport));
com!(set_render_state, 57, (state: u32, value: u32));
com!(create_state_block, 59, (kind: u32, out: *mut Object));
com!(set_texture, 65, (stage: u32, texture: Object));
com!(set_sampler_state, 69, (sampler: u32, state: u32, value: u32));
com!(draw_primitive_up, 83, (kind: u32, count: u32, data: *const c_void, stride: u32));
com!(create_vertex_declaration, 86, (elements: *const VertexElement, out: *mut Object));
com!(set_vertex_declaration, 87, (decl: Object));
com!(create_vertex_shader, 91, (code: *const u32, out: *mut Object));
com!(set_vertex_shader, 92, (shader: Object));
com!(set_vs_constants, 94, (start: u32, data: *const f32, count: u32));
com!(create_pixel_shader, 106, (code: *const u32, out: *mut Object));
com!(set_pixel_shader, 107, (shader: Object));
com!(set_ps_constants, 109, (start: u32, data: *const f32, count: u32));
// IDirect3DTexture9
com!(texture_level_desc, 17, (level: u32, out: *mut SurfaceDesc));
com!(texture_surface, 18, (level: u32, out: *mut Object));
// IDirect3DSurface9
com!(surface_desc, 12, (out: *mut SurfaceDesc));
// IDirect3DStateBlock9
com!(capture, 4, ());
com!(apply, 5, ());

/// Releases `*object` if set and clears it.
///
/// # Safety
/// `*object` must be null or a live COM object.
pub unsafe fn clear(object: &mut Object) {
    if !object.is_null() {
        // SAFETY: the caller's promise.
        unsafe { release(*object) };
        *object = std::ptr::null_mut();
    }
}
