//! Drawing of mods over the frame (stage 116).
//!
//! The place: `mov eax, [0x01205750]` at `0x00608E9E`, in the copy and
//! `Present` of the frame `0x00608E90` — after the copy of the scene into
//! the back buffer (and doctor_render's post-effects, which replace that
//! copy), right before `Present`, only when the game has a window. The jump
//! is made when the first mod subscribes, from `doctor_mod_init` only: the
//! five bytes are code that runs every frame, and the mods are initialised
//! on the main thread, between the frames it presents itself at the start,
//! before the thread of the frames is started (the device exists by then:
//! `docs/analysis/game-bench.md` §59) — so no thread runs the bytes while
//! they change. By then every `.asi` has checked its build, so a
//! doctor_render that still checks these bytes has already said yes.
//!
//! Around the listeners the platform keeps the game's state: a state block
//! of `D3DSBT_ALL` (captured, then applied), render target 0 and the depth
//! surface; the listeners draw into the back buffer, without depth, inside
//! `BeginScene`. The block is released when the device is lost
//! ([`crate::frame`]) and made again on the next frame.
//!
//! doctor_render watches the state of the device through its own hooks of
//! the device's methods: a state block made or applied behind them would
//! stop its draw filter for good. When it is loaded, the platform tells it
//! through its exports `doctor_render_own_state_block` and
//! `doctor_render_state_changed`.

use std::arch::global_asm;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use doctor_sdk::abi::{self, Overlay, OverlayFn};
use doctor_toolkit_live::platform::addr;
use doctor_toolkit_live::render::MODULE_FILE as RENDER_FILE;

use crate::d3d::{self, Object};
use crate::win;

const MAX: usize = 16;

struct Listener {
    id: i32,
    f: OverlayFn,
    user: *mut c_void,
    order: i32,
    shown: bool,
}

struct Listeners(Vec<Listener>);
// SAFETY: `user` is a pointer of a mod, only handed back to it.
unsafe impl Send for Listeners {}

static LISTENERS: Mutex<Listeners> = Mutex::new(Listeners(Vec::new()));
/// Some listener is shown: the frame does anything at all.
static SHOWN: AtomicBool = AtomicBool::new(false);
static HOOKED: AtomicBool = AtomicBool::new(false);
/// The overlay gave up (no state block): it stays off.
static FAILED: AtomicBool = AtomicBool::new(false);
static FRAME: AtomicU32 = AtomicU32::new(0);

/// The state block of the platform; made on the thread of the frames.
struct Block(Object);
// SAFETY: a COM object of the game's device, used under the mutex.
unsafe impl Send for Block {}
static BLOCK: Mutex<Block> = Mutex::new(Block(std::ptr::null_mut()));

/// The two exports of doctor_render, when it is loaded.
#[derive(Clone, Copy)]
struct Render {
    own_state_block: Option<unsafe extern "C" fn(i32)>,
    state_changed: Option<unsafe extern "C" fn()>,
}

static RENDER: Mutex<Option<Render>> = Mutex::new(None);

fn render() -> Render {
    let Ok(mut r) = RENDER.lock() else {
        return Render {
            own_state_block: None,
            state_changed: None,
        };
    };
    *r.get_or_insert_with(|| {
        let module = win::modules().into_iter().find(|&base| {
            win::module_path(base)
                .rsplit(['\\', '/'])
                .next()
                .is_some_and(|n| n.eq_ignore_ascii_case(RENDER_FILE))
        });
        let export = |name: &core::ffi::CStr| -> *mut c_void {
            match module {
                // SAFETY: a module of this process; a zero-terminated name.
                Some(base) => unsafe {
                    win::GetProcAddress(base as usize as *mut c_void, name.as_ptr().cast())
                },
                None => std::ptr::null_mut(),
            }
        };
        let own = export(c"doctor_render_own_state_block");
        let changed = export(c"doctor_render_state_changed");
        crate::log(&match module {
            None => "overlay: doctor_render is not loaded".to_owned(),
            Some(_) if own.is_null() || changed.is_null() => {
                "overlay: doctor_render without the exports of the state (an older build): its draw filter stops when the overlay first draws".to_owned()
            }
            Some(_) => "overlay: doctor_render told of the state of the overlay".to_owned(),
        });
        // SAFETY: the exports have these signatures (doctor_render's lib.rs).
        unsafe {
            Render {
                own_state_block: (!own.is_null())
                    .then(|| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(i32)>(own)),
                state_changed: (!changed.is_null())
                    .then(|| std::mem::transmute::<*mut c_void, unsafe extern "C" fn()>(changed)),
            }
        }
    })
}

fn update_shown(list: &Listeners) {
    SHOWN.store(list.0.iter().any(|l| l.shown), Ordering::Relaxed);
}

// --- The hook ---------------------------------------------------------------------

// The thunk the jump at 0x00608E9E goes to: keeps every register and the
// flags, draws, runs the instruction the jump took and goes on.
global_asm!(
    ".globl _doctor_platform_overlay_thunk",
    "_doctor_platform_overlay_thunk:",
    "pushfd",
    "pushad",
    "call {dispatch}",
    "popad",
    "popfd",
    "mov eax, dword ptr [{device}]",
    "push {resume}",
    "ret",
    dispatch = sym dispatch,
    device = const addr::DEVICE,
    resume = const addr::OVERLAY_RESUME,
);

unsafe extern "C" {
    fn doctor_platform_overlay_thunk();
}

fn install() -> i32 {
    if HOOKED.load(Ordering::Relaxed) {
        return abi::OK;
    }
    let thunk = doctor_platform_overlay_thunk as unsafe extern "C" fn() as usize as u32;
    let mut code = [0u8; addr::OVERLAY_HOOK_LEN as usize];
    code[0] = 0xE9;
    code[1..5].copy_from_slice(&thunk.wrapping_sub(addr::OVERLAY_HOOK + 5).to_le_bytes());
    // SAFETY: one whole instruction, checked by the edition check; patched
    // from doctor_mod_init on the main thread, before the thread of the
    // frames starts.
    if !unsafe { win::patch(addr::OVERLAY_HOOK, &code) } {
        return abi::ERR_ENGINE;
    }
    HOOKED.store(true, Ordering::Relaxed);
    // SAFETY: a pointer in the data of the game.
    let device = unsafe { win::read::<u32>(addr::DEVICE) };
    crate::log(&format!(
        "overlay: hook at {:#010x}; the device {}",
        addr::OVERLAY_HOOK,
        if device == 0 {
            "does not exist yet"
        } else {
            "exists already"
        }
    ));
    abi::OK
}

extern "C" fn dispatch() {
    if !SHOWN.load(Ordering::Relaxed) || FAILED.load(Ordering::Relaxed) {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: a byte and a pointer in the data of the game.
        let (lost, device) = unsafe {
            (
                win::read::<u8>(addr::DEVICE_LOST_FLAG),
                win::read::<u32>(addr::DEVICE) as usize as Object,
            )
        };
        if lost != 0 || device.is_null() {
            return;
        }
        let listeners: Vec<(OverlayFn, *mut c_void)> = match LISTENERS.lock() {
            Ok(l) => {
                l.0.iter()
                    .filter(|l| l.shown)
                    .map(|l| (l.f, l.user))
                    .collect()
            }
            Err(_) => return,
        };
        if listeners.is_empty() {
            return;
        }
        let Ok(mut block) = BLOCK.lock() else {
            return;
        };
        // SAFETY: the game's device on the thread that draws with it.
        unsafe { draw(device, &mut block.0, &listeners) };
    });
}

/// Saves the state, sets the back buffer, calls the listeners, puts the
/// state back.
///
/// # Safety
/// `device` is the game's device, on a thread that draws with it.
unsafe fn draw(device: Object, block: &mut Object, listeners: &[(OverlayFn, *mut c_void)]) {
    let render = render();
    // SAFETY: the caller's promise; every object is released below.
    unsafe {
        if block.is_null() {
            if let Some(own) = render.own_state_block {
                own(1);
            }
            let made = d3d::create_state_block(device, d3d::D3DSBT_ALL, block);
            if let Some(own) = render.own_state_block {
                own(0);
            }
            if made < 0 || block.is_null() {
                *block = std::ptr::null_mut();
                FAILED.store(true, Ordering::Relaxed);
                crate::log(&format!(
                    "overlay: the device made no state block ({made:#010x}); the overlay is off"
                ));
                return;
            }
        }
        let mut back: Object = std::ptr::null_mut();
        if d3d::get_back_buffer(device, 0, 0, d3d::D3DBACKBUFFER_TYPE_MONO, &mut back) < 0
            || back.is_null()
        {
            return;
        }
        let mut desc = d3d::SurfaceDesc::default();
        d3d::surface_desc(back, &mut desc);
        let mut old_target: Object = std::ptr::null_mut();
        let mut old_depth: Object = std::ptr::null_mut();
        d3d::get_render_target(device, 0, &mut old_target);
        if d3d::get_depth_stencil(device, &mut old_depth) < 0 {
            old_depth = std::ptr::null_mut();
        }
        d3d::capture(*block);
        d3d::set_render_target(device, 0, back);
        d3d::set_depth_stencil(device, std::ptr::null_mut());
        let viewport = d3d::Viewport {
            x: 0,
            y: 0,
            width: desc.width,
            height: desc.height,
            min_z: 0.0,
            max_z: 1.0,
        };
        d3d::set_viewport(device, &viewport);
        let began = d3d::begin_scene(device) >= 0;
        let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
        let overlay = Overlay {
            size: std::mem::size_of::<Overlay>() as u32,
            device,
            width: desc.width,
            height: desc.height,
            frame,
            main_thread: u32::from(win::GetCurrentThreadId() == crate::main_thread()),
        };
        for &(f, user) in listeners {
            // A function of a mod (extern "C": it cannot unwind into here).
            f(user, &overlay);
        }
        if began {
            d3d::end_scene(device);
        }
        if !old_target.is_null() {
            d3d::set_render_target(device, 0, old_target);
        }
        d3d::set_depth_stencil(device, old_depth);
        d3d::apply(*block);
        if let Some(changed) = render.state_changed {
            changed();
        }
        d3d::clear(&mut old_target);
        d3d::clear(&mut old_depth);
        d3d::release(back);
        if frame == 1 {
            crate::log(&format!(
                "overlay: first frame {}x{} on the {} thread",
                desc.width,
                desc.height,
                if overlay.main_thread == 1 {
                    "main"
                } else {
                    "presenting"
                }
            ));
        }
    }
}

/// Before `Reset`: the state block goes (it is made again after).
pub fn device_lost() {
    if let Ok(mut block) = BLOCK.lock() {
        // SAFETY: the block of the platform, or null.
        unsafe { d3d::clear(&mut block.0) };
    }
}

// --- The functions of the table ---------------------------------------------------

pub fn listen(f: OverlayFn, user: *mut c_void, order: i32) -> i32 {
    if !crate::in_init() {
        crate::log("overlay_listen refused: only from doctor_mod_init");
        return abi::ERR_STATE;
    }
    let Ok(mut list) = LISTENERS.lock() else {
        return abi::ERR_ENGINE;
    };
    if list.0.len() >= MAX {
        return abi::ERR_FULL;
    }
    let hooked = install();
    if hooked != abi::OK {
        return hooked;
    }
    if crate::frame::register_device().is_err() {
        crate::log(
            "overlay: without the events of the device the state block is not released before Reset",
        );
    }
    let id = crate::frame::next_id();
    list.0.push(Listener {
        id,
        f,
        user,
        order,
        shown: true,
    });
    list.0.sort_by_key(|l| l.order);
    update_shown(&list);
    id
}

pub fn show(id: i32, on: u32) -> i32 {
    let Ok(mut list) = LISTENERS.lock() else {
        return abi::ERR_ENGINE;
    };
    let Some(l) = list.0.iter_mut().find(|l| l.id == id) else {
        return abi::ERR_ARGUMENT;
    };
    l.shown = on != 0;
    update_shown(&list);
    abi::OK
}

pub fn unlisten(id: i32) -> i32 {
    let Ok(mut list) = LISTENERS.lock() else {
        return abi::ERR_ENGINE;
    };
    let before = list.0.len();
    list.0.retain(|l| l.id != id);
    if list.0.len() == before {
        return abi::ERR_ARGUMENT;
    }
    update_shown(&list);
    abi::OK
}
