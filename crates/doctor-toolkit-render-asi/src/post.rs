//! Post-effects over the finished frame (`renderer.md` §2, §8).
//!
//! The game draws its scene — and its interface on top — into a colour
//! target of its own (`*0x012058A4`) and, on the render thread right before
//! `Present`, copies it into the back buffer (`0x00608DC0`,
//! `D3DXLoadSurfaceFromSurface`). The module stands in that copy: it draws
//! the target into the back buffer with its own shaders — FXAA, sharpening,
//! or both through a texture of its own — and puts every state of the
//! device back (a state block of `D3DSBT_ALL`, the render target and the
//! depth buffer). When anything is missing it falls back to the game's
//! copy. The interface is already in the target, so it passes through the
//! filters too.
//!
//! The shaders are HLSL of this repository (`shaders/`), compiled ahead to
//! Direct3D 9 bytecode (`tools/build-render-shaders.sh`).
//!
//! Resources of the `DEFAULT` pool (the state block, the texture between
//! two passes) must go before `IDirect3DDevice9::Reset`: the module stands
//! in the game's release of its targets before a reset (`0x00607E10`) and
//! releases them there; they are made again on the next frame.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use doctor_toolkit_live::render::{RenderConfig, addr};

use crate::d3d::{self, Object};
use crate::win;

static VS: &[u8] = include_bytes!("../shaders/post_vs.cso");
static FXAA: &[u8] = include_bytes!("../shaders/fxaa.cso");
static CAS: &[u8] = include_bytes!("../shaders/cas.cso");

/// What the passes need, made on the device of the game.
struct Resources {
    device: usize,
    vs: Object,
    decl: Object,
    fxaa: Object,
    cas: Object,
    state: Object,
    /// Texture between FXAA and sharpening, and its size.
    between: Object,
    between_size: (u32, u32),
}

// SAFETY: the objects are used only on the render thread, under the lock.
unsafe impl Send for Resources {}

static RESOURCES: Mutex<Option<Resources>> = Mutex::new(None);
static FXAA_ON: AtomicBool = AtomicBool::new(false);
/// `-1 / lerp(8, 5, sharpness)` as `f32` bits, 0 when off.
static SHARPEN: AtomicU32 = AtomicU32::new(0);
/// Frames drawn by the module, and failures (after a few, it stops).
static DRAWN: AtomicU32 = AtomicU32::new(0);
static FAILED: AtomicU32 = AtomicU32::new(0);
static DISABLED: AtomicBool = AtomicBool::new(false);

const QUAD: [[f32; 6]; 4] = [
    [-1.0, 1.0, 0.0, 1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0, 1.0, 1.0, 0.0],
    [-1.0, -1.0, 0.0, 1.0, 0.0, 1.0],
    [1.0, -1.0, 0.0, 1.0, 1.0, 1.0],
];

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn fail(why: &str) -> bool {
    let n = FAILED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 3 {
        crate::log(&format!("post: {why}; the game's copy instead"));
    }
    if n >= 20 && !DISABLED.swap(true, Ordering::Relaxed) {
        crate::log("post: off after 20 failures");
    }
    false
}

impl Resources {
    /// Shaders and the declaration: kept over resets.
    unsafe fn new(device: Object) -> Result<Self, String> {
        let mut r = Self {
            device: device as usize,
            vs: std::ptr::null_mut(),
            decl: std::ptr::null_mut(),
            fxaa: std::ptr::null_mut(),
            cas: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            between: std::ptr::null_mut(),
            between_size: (0, 0),
        };
        let elements = [
            d3d::VertexElement {
                stream: 0,
                offset: 0,
                kind: 3, // FLOAT4
                method: 0,
                usage: 0, // POSITION
                usage_index: 0,
            },
            d3d::VertexElement {
                stream: 0,
                offset: 16,
                kind: 1, // FLOAT2
                method: 0,
                usage: 5, // TEXCOORD
                usage_index: 0,
            },
            d3d::VertexElement {
                stream: 0xFF,
                offset: 0,
                kind: 17, // UNUSED: the end
                method: 0,
                usage: 0,
                usage_index: 0,
            },
        ];
        // SAFETY: the game's device; outputs are released by `drop`.
        unsafe {
            if d3d::create_vertex_declaration(device, elements.as_ptr(), &mut r.decl) < 0 {
                return Err("CreateVertexDeclaration failed".into());
            }
            if d3d::create_vertex_shader(device, words(VS).as_ptr(), &mut r.vs) < 0 {
                return Err("CreateVertexShader failed".into());
            }
            if d3d::create_pixel_shader(device, words(FXAA).as_ptr(), &mut r.fxaa) < 0 {
                return Err("CreatePixelShader (FXAA) failed".into());
            }
            if d3d::create_pixel_shader(device, words(CAS).as_ptr(), &mut r.cas) < 0 {
                return Err("CreatePixelShader (sharpening) failed".into());
            }
        }
        Ok(r)
    }

    /// The resources of the `DEFAULT` pool.
    unsafe fn release_default(&mut self) {
        // SAFETY: objects made on this device.
        unsafe {
            d3d::clear(&mut self.state);
            d3d::clear(&mut self.between);
        }
        self.between_size = (0, 0);
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        // SAFETY: objects made on this device.
        unsafe {
            self.release_default();
            d3d::clear(&mut self.vs);
            d3d::clear(&mut self.decl);
            d3d::clear(&mut self.fxaa);
            d3d::clear(&mut self.cas);
        }
    }
}

/// One full-screen pass of `shader` from `source` into `target`.
unsafe fn pass(
    device: Object,
    shader: Object,
    source: Object,
    source_size: (u32, u32),
    target: Object,
    target_size: (u32, u32),
) -> bool {
    let (w, h) = target_size;
    let (sw, sh) = (source_size.0.max(1) as f32, source_size.1.max(1) as f32);
    let viewport = d3d::Viewport {
        x: 0,
        y: 0,
        width: w,
        height: h,
        min_z: 0.0,
        max_z: 1.0,
    };
    let half = [-1.0 / w.max(1) as f32, 1.0 / h.max(1) as f32, 0.0, 0.0];
    let texel = [1.0 / sw, 1.0 / sh, sw, sh];
    let strength = [
        f32::from_bits(SHARPEN.load(Ordering::Relaxed)),
        0.0,
        0.0,
        0.0,
    ];
    // SAFETY: the game's device and objects made on it.
    unsafe {
        if d3d::set_render_target(device, 0, target) < 0 {
            return false;
        }
        d3d::set_viewport(device, &viewport);
        d3d::set_vs_constants(device, 0, half.as_ptr(), 1);
        d3d::set_ps_constants(device, 0, texel.as_ptr(), 1);
        d3d::set_ps_constants(device, 1, strength.as_ptr(), 1);
        d3d::set_texture(device, 0, source);
        d3d::set_pixel_shader(device, shader);
        d3d::draw_primitive_up(
            device,
            d3d::D3DPT_TRIANGLESTRIP,
            2,
            QUAD.as_ptr() as *const c_void,
            24,
        ) >= 0
    }
}

/// The states every pass needs.
unsafe fn states(device: Object, r: &Resources) {
    // SAFETY: the game's device and objects made on it.
    unsafe {
        for (state, value) in [
            (d3d::RS_ZENABLE, 0),
            (d3d::RS_ZWRITEENABLE, 0),
            (d3d::RS_FILLMODE, 3),
            (d3d::RS_ALPHATESTENABLE, 0),
            (d3d::RS_ALPHABLENDENABLE, 0),
            (d3d::RS_SEPARATEALPHABLENDENABLE, 0),
            (d3d::RS_CULLMODE, 1),
            (d3d::RS_FOGENABLE, 0),
            (d3d::RS_STENCILENABLE, 0),
            (d3d::RS_CLIPPLANEENABLE, 0),
            (d3d::RS_SCISSORTESTENABLE, 0),
            (d3d::RS_COLORWRITEENABLE, 0xF),
            (d3d::RS_SRGBWRITEENABLE, 0),
        ] {
            d3d::set_render_state(device, state, value);
        }
        for (state, value) in [
            (d3d::SAMP_ADDRESSU, d3d::TADDRESS_CLAMP),
            (d3d::SAMP_ADDRESSV, d3d::TADDRESS_CLAMP),
            (d3d::SAMP_MAGFILTER, d3d::TEXF_LINEAR),
            (d3d::SAMP_MINFILTER, d3d::TEXF_LINEAR),
            (d3d::SAMP_MIPFILTER, d3d::TEXF_NONE),
            (d3d::SAMP_MIPMAPLODBIAS, 0),
            (d3d::SAMP_MAXMIPLEVEL, 0),
            (d3d::SAMP_SRGBTEXTURE, 0),
        ] {
            d3d::set_sampler_state(device, 0, state, value);
        }
        d3d::set_vertex_declaration(device, r.decl);
        d3d::set_vertex_shader(device, r.vs);
    }
}

/// Draws the frame through the filters; `false`: nothing drawn, the
/// game's copy should run.
unsafe fn draw() -> bool {
    if DISABLED.load(Ordering::Relaxed) {
        return false;
    }
    let fxaa = FXAA_ON.load(Ordering::Relaxed);
    let sharpen = SHARPEN.load(Ordering::Relaxed) != 0;
    // SAFETY: data of the game proved by the checked places.
    let (device, scene) = unsafe {
        let device = win::read::<u32>(addr::DEVICE) as usize as Object;
        let index = win::read::<u32>(addr::SCENE_TARGET);
        if device.is_null() || index >= addr::TEXTURE_COUNT {
            return false;
        }
        let entry = addr::TEXTURES + index * addr::TEXTURE_ENTRY;
        (
            device,
            win::read::<u32>(entry + addr::TEXTURE_PTR) as usize as Object,
        )
    };
    if scene.is_null() {
        return false;
    }
    let Ok(mut guard) = RESOURCES.lock() else {
        return false;
    };
    if guard.as_ref().is_some_and(|r| r.device != device as usize) {
        // Another device: the old one is gone with its objects.
        if let Some(old) = guard.take() {
            std::mem::forget(old);
        }
    }
    if guard.is_none() {
        // SAFETY: the game's device.
        match unsafe { Resources::new(device) } {
            Ok(r) => *guard = Some(r),
            Err(why) => {
                DISABLED.store(true, Ordering::Relaxed);
                crate::log(&format!("post: {why}; off"));
                return false;
            }
        }
    }
    let Some(r) = guard.as_mut() else {
        return false;
    };
    // SAFETY: the game's device on its render thread, objects made on it;
    // every reference taken here is released before returning.
    unsafe {
        if r.state.is_null() && d3d::create_state_block(device, d3d::D3DSBT_ALL, &mut r.state) < 0 {
            r.state = std::ptr::null_mut();
            DISABLED.store(true, Ordering::Relaxed);
            crate::log("post: CreateStateBlock(D3DSBT_ALL) failed (a pure device?); off");
            return false;
        }
        let mut scene_desc = d3d::SurfaceDesc::default();
        if d3d::texture_level_desc(scene, 0, &mut scene_desc) < 0 {
            return fail("the scene target has no level 0");
        }
        let scene_size = (scene_desc.width, scene_desc.height);
        let mut back: Object = std::ptr::null_mut();
        if d3d::get_back_buffer(device, 0, 0, 0, &mut back) < 0 || back.is_null() {
            return fail("no back buffer");
        }
        let mut back_desc = d3d::SurfaceDesc::default();
        d3d::surface_desc(back, &mut back_desc);
        let back_size = (back_desc.width, back_desc.height);
        if fxaa && sharpen && r.between_size != scene_size {
            d3d::clear(&mut r.between);
            if d3d::create_texture(
                device,
                scene_size.0,
                scene_size.1,
                1,
                d3d::D3DUSAGE_RENDERTARGET,
                d3d::D3DFMT_A8R8G8B8,
                d3d::D3DPOOL_DEFAULT,
                &mut r.between,
                std::ptr::null_mut(),
            ) < 0
            {
                r.between = std::ptr::null_mut();
                d3d::release(back);
                return fail("cannot make the texture between the passes");
            }
            r.between_size = scene_size;
        }
        let mut old_target: Object = std::ptr::null_mut();
        let mut old_depth: Object = std::ptr::null_mut();
        d3d::get_render_target(device, 0, &mut old_target);
        if d3d::get_depth_stencil(device, &mut old_depth) < 0 {
            old_depth = std::ptr::null_mut();
        }
        d3d::capture(r.state);
        let began = d3d::begin_scene(device) >= 0;
        d3d::set_depth_stencil(device, std::ptr::null_mut());
        states(device, r);
        let ok = if fxaa && sharpen {
            let mut between_surface: Object = std::ptr::null_mut();
            let ok = d3d::texture_surface(r.between, 0, &mut between_surface) >= 0
                && pass(
                    device,
                    r.fxaa,
                    scene,
                    scene_size,
                    between_surface,
                    scene_size,
                )
                && pass(device, r.cas, r.between, scene_size, back, back_size);
            d3d::clear(&mut between_surface);
            ok
        } else if fxaa {
            pass(device, r.fxaa, scene, scene_size, back, back_size)
        } else {
            pass(device, r.cas, scene, scene_size, back, back_size)
        };
        if began {
            d3d::end_scene(device);
        }
        d3d::set_texture(device, 0, std::ptr::null_mut());
        if !old_target.is_null() {
            d3d::set_render_target(device, 0, old_target);
        }
        d3d::set_depth_stencil(device, old_depth);
        d3d::apply(r.state);
        d3d::clear(&mut old_target);
        d3d::clear(&mut old_depth);
        d3d::release(back);
        if !ok {
            return fail("a pass failed");
        }
        if DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
            crate::log(&format!(
                "post: {}{}{} over the frame {}x{} -> back buffer {}x{}",
                if fxaa { "FXAA" } else { "" },
                if fxaa && sharpen { " + " } else { "" },
                if sharpen { "sharpening" } else { "" },
                scene_size.0,
                scene_size.1,
                back_size.0,
                back_size.1
            ));
        }
        true
    }
}

/// Stands in the call of the copy `0x00608DC0` in `0x00608E90`.
extern "C" fn copy() {
    // SAFETY: on the render thread, where the game itself draws.
    let drawn = std::panic::catch_unwind(|| unsafe { draw() }).unwrap_or(false);
    if !drawn {
        // SAFETY: the game's copy, cdecl, no arguments (checked).
        let original: extern "C" fn() = unsafe { std::mem::transmute(addr::COPY as usize) };
        original();
    }
}

/// Stands in the call of the release of the targets `0x00607E10` before a
/// reset of the device: releases the module's resources of the `DEFAULT`
/// pool first.
extern "C" fn release(arg: u32) {
    let _ = std::panic::catch_unwind(|| {
        if let Ok(mut guard) = RESOURCES.lock()
            && let Some(r) = guard.as_mut()
        {
            // SAFETY: objects made on the game's device.
            unsafe { r.release_default() };
        }
    });
    // SAFETY: the game's release, cdecl, one argument (checked).
    let original: extern "C" fn(u32) = unsafe { std::mem::transmute(addr::RELEASE as usize) };
    original(arg);
}

pub fn install(config: &RenderConfig, done: &mut Vec<String>) {
    if !config.post() {
        return;
    }
    FXAA_ON.store(config.fxaa, Ordering::Relaxed);
    if config.sharpen > 0 {
        let s = config.sharpen.min(100) as f32 / 100.0;
        let peak = -1.0 / (8.0 - 3.0 * s);
        SHARPEN.store(peak.to_bits(), Ordering::Relaxed);
    }
    // SAFETY: checked call instructions.
    unsafe {
        let a = win::redirect_call(addr::RELEASE_CALL, release as extern "C" fn(u32) as usize);
        let b = a && win::redirect_call(addr::COPY_CALL, copy as extern "C" fn() as usize);
        if b {
            done.push(format!(
                "post-effects{}{} instead of the copy of the frame (0x00608E90, release 0x00608257)",
                if config.fxaa { ", FXAA" } else { "" },
                if config.sharpen > 0 {
                    format!(", sharpening {}%", config.sharpen)
                } else {
                    String::new()
                }
            ));
        }
    }
}
