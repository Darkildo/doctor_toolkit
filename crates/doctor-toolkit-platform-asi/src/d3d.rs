//! The few Direct3D 9 calls of the overlay, through the vtables (the module
//! links no `d3d9.lib`: the device is the game's). The slots are the ones
//! of doctor_render's `d3d.rs`.

use std::ffi::c_void;

pub type Object = *mut c_void;

pub const D3DSBT_ALL: u32 = 1;
pub const D3DBACKBUFFER_TYPE_MONO: u32 = 0;

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
    ($name:ident, $index:expr, ($($arg:ident: $ty:ty),*)) => {
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
com!(set_render_target, 37, (index: u32, surface: Object));
com!(get_render_target, 38, (index: u32, out: *mut Object));
com!(set_depth_stencil, 39, (surface: Object));
com!(get_depth_stencil, 40, (out: *mut Object));
com!(begin_scene, 41, ());
com!(end_scene, 42, ());
com!(set_viewport, 47, (viewport: *const Viewport));
com!(create_state_block, 59, (kind: u32, out: *mut Object));
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
