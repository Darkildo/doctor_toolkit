//! The input gate (stage 117): what the player presses, for the mods, and
//! nothing pressed for the game while a mod holds a capture.
//!
//! The game polls its devices in one place, once a frame on the main thread
//! and in both main loops (`0x00B950E0`, from `0x0068BC12`): XInput through
//! `XInputGetState` (`call` at `0x00B941EB`) and DirectInput through
//! `GetDeviceState` of each device (`0x00B9519D` keyboard, `0x00B95221`
//! mouse, `0x00B952BF` joystick). The module replaces the function each of
//! those calls reaches with its own: it calls the real one, keeps the state
//! for [`read`] and, while a capture is held, hands the game a state with
//! nothing pressed — the game's menus, the pause menu among them, read the
//! same devices. When the last capture is let go, the keys, buttons and
//! triggers held at that moment stay hidden from the game until they are
//! released, so the key that closed a mod's menu does nothing in the game.
//! Keys a mod reserves ([`reserve`]) never reach the game: the poll comes
//! before the tick in which a mod sees its key, so without that the game
//! would see the key that opens a menu for one frame.
//!
//! The cursor of the game's menus is not a device either: the window's
//! `WM_MOUSEMOVE` handler adds up the movement, and the menus take it
//! through `0x0069EB00` (`docs/analysis/game-bench.md` §61); in one mode
//! (`0x0112A06D`) the game also asks `GetCursorPos` (`0x0069E6AF`). While a
//! capture is held the module throws the movement away and keeps the point
//! `GetCursorPos` gives where it was when the capture began, so the menus
//! under a mod's menu do not follow the mouse.
//!
//! The game reads every poll in full (it does not look at
//! `dwPacketNumber`), so a state with nothing pressed takes effect at once.
//! It turns the axes of a DirectInput joystick from 0..65535 into
//! -1000..1000, so "nothing" for them is the middle, 32767.

use std::arch::global_asm;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use doctor_sdk::abi::{self, Input, Pad};
use doctor_toolkit_live::platform::addr;

use crate::win;

/// Sizes the game asks `GetDeviceState` for.
const KEYBOARD: u32 = 256;
/// `DIMOUSESTATE2`: three axes, eight buttons.
const MOUSE: u32 = 0x14;
/// `DIJOYSTATE2`.
const JOYSTICK: u32 = 0x110;
const JOYSTICK_AXES: usize = 8;
const JOYSTICK_POVS: usize = 4;
const JOYSTICK_BUTTONS_AT: usize = 48;
const JOYSTICK_BUTTONS: usize = 128;
const JOYSTICK_MIDDLE: i32 = 32767;
/// A trigger above this counts as held.
const TRIGGER_HELD: u8 = 30;

/// What stays hidden from the game after the last capture, until released.
struct Held {
    keys: [bool; 256],
    mouse: [bool; 8],
    joystick: [bool; JOYSTICK_BUTTONS],
    pad_buttons: [u16; 4],
    pad_triggers: [[bool; 2]; 4],
}

struct State {
    input: Input,
    held: Held,
    /// All the buttons of the mouse and of the joystick, for [`Held`]
    /// (the [`Input`] of the mods keeps the first 4 and 32).
    mouse_buttons: [u8; 8],
    joystick_buttons: [u8; JOYSTICK_BUTTONS],
    /// Keys reserved by mods, counted.
    reserved: [u16; 256],
    /// The point the cursor of the game's menus stays at while captured.
    cursor: Option<[i32; 2]>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static CAPTURES: AtomicI32 = AtomicI32::new(0);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    let mut guard = STATE.lock().ok()?;
    let state = guard.get_or_insert_with(|| State {
        input: Input::default(),
        held: Held {
            keys: [false; 256],
            mouse: [false; 8],
            joystick: [false; JOYSTICK_BUTTONS],
            pad_buttons: [0; 4],
            pad_triggers: [[false; 2]; 4],
        },
        mouse_buttons: [0; 8],
        joystick_buttons: [0; JOYSTICK_BUTTONS],
        reserved: [0; 256],
        cursor: None,
    });
    Some(f(state))
}

fn captured() -> bool {
    CAPTURES.load(Ordering::Relaxed) > 0
}

/// A pressed bit of DirectInput hidden while held; the mark goes when the
/// real one is released.
fn hide_held(byte: &mut u8, held: &mut bool) {
    if *held {
        if *byte & 0x80 == 0 {
            *held = false;
        } else {
            *byte = 0;
        }
    }
}

// --- DirectInput ------------------------------------------------------------------

/// What the three places of the poll call instead of `GetDeviceState`
/// (`IDirectInputDevice8::GetDeviceState`, slot 9, stdcall).
unsafe extern "system" fn get_device_state(device: *mut c_void, size: u32, data: *mut u8) -> i32 {
    // SAFETY: the device the game polls; slot 9 of its table.
    let result = unsafe {
        let real: unsafe extern "system" fn(*mut c_void, u32, *mut u8) -> i32 =
            std::mem::transmute(*(*(device as *const *const usize)).add(9));
        real(device, size, data)
    };
    if result != 0 || data.is_null() {
        if size == JOYSTICK {
            with(|s| s.input.joystick = 0);
        }
        return result;
    }
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `size` bytes the device has just written.
        let data = unsafe { std::slice::from_raw_parts_mut(data, size as usize) };
        let closed = captured();
        with(|s| {
            s.input.serial = s.input.serial.wrapping_add(1);
            match size {
                KEYBOARD => {
                    s.input.keys.copy_from_slice(data);
                    if closed {
                        data.fill(0);
                    } else {
                        for (byte, held) in data.iter_mut().zip(s.held.keys.iter_mut()) {
                            hide_held(byte, held);
                        }
                    }
                    for (byte, reserved) in data.iter_mut().zip(s.reserved) {
                        if reserved > 0 {
                            *byte = 0;
                        }
                    }
                }
                MOUSE => {
                    let word = |i: usize| {
                        i32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]])
                    };
                    s.input.mouse_dx = word(0);
                    s.input.mouse_dy = word(4);
                    s.input.mouse_wheel = word(8);
                    s.input.mouse_buttons.copy_from_slice(&data[12..16]);
                    s.mouse_buttons.copy_from_slice(&data[12..20]);
                    if closed {
                        data.fill(0);
                    } else {
                        for (byte, held) in data[12..20].iter_mut().zip(s.held.mouse.iter_mut()) {
                            hide_held(byte, held);
                        }
                    }
                }
                JOYSTICK => {
                    let word = |i: usize| {
                        i32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]])
                    };
                    s.input.joystick = 1;
                    s.input.joystick_x = word(0);
                    s.input.joystick_y = word(4);
                    s.input.joystick_pov = word(32) as u32;
                    s.input
                        .joystick_buttons
                        .copy_from_slice(&data[JOYSTICK_BUTTONS_AT..JOYSTICK_BUTTONS_AT + 32]);
                    let buttons = JOYSTICK_BUTTONS_AT..JOYSTICK_BUTTONS_AT + JOYSTICK_BUTTONS;
                    s.joystick_buttons.copy_from_slice(&data[buttons.clone()]);
                    if closed {
                        for axis in 0..JOYSTICK_AXES {
                            data[axis * 4..axis * 4 + 4]
                                .copy_from_slice(&JOYSTICK_MIDDLE.to_le_bytes());
                        }
                        for pov in 0..JOYSTICK_POVS {
                            let at = 32 + pov * 4;
                            data[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                        }
                        data[buttons].fill(0);
                    } else {
                        for (byte, held) in data[buttons].iter_mut().zip(s.held.joystick.iter_mut())
                        {
                            hide_held(byte, held);
                        }
                    }
                }
                _ => {}
            }
        });
    });
    result
}

// The keyboard: `mov ecx, [edx+0x24]; add esp, 8` at 0x00B9518C; the game
// then pushes the arguments and calls ECX. The code after addresses
// [esp+0x2C], so this is reached by a jump and leaves by one.
global_asm!(
    ".globl _doctor_platform_input_keyboard",
    "_doctor_platform_input_keyboard:",
    "lea ecx, [{gds}]",
    "add esp, 8",
    "push {resume}",
    "ret",
    gds = sym get_device_state,
    resume = const addr::INPUT_KEYBOARD_RESUME,
);

// The mouse: `mov eax, [ecx+0x24]; call eax` at 0x00B9521E, the arguments
// already pushed.
global_asm!(
    ".globl _doctor_platform_input_mouse",
    "_doctor_platform_input_mouse:",
    "lea eax, [{gds}]",
    "call eax",
    "push {resume}",
    "ret",
    gds = sym get_device_state,
    resume = const addr::INPUT_MOUSE_RESUME,
);

// The joystick: `mov ecx, [edx+0x24]; lea eax, [esp+0x2C]` at 0x00B952B1.
global_asm!(
    ".globl _doctor_platform_input_joystick",
    "_doctor_platform_input_joystick:",
    "lea ecx, [{gds}]",
    "lea eax, [esp + 0x2C]",
    "push {resume}",
    "ret",
    gds = sym get_device_state,
    resume = const addr::INPUT_JOYSTICK_RESUME,
);

unsafe extern "C" {
    fn doctor_platform_input_keyboard();
    fn doctor_platform_input_mouse();
    fn doctor_platform_input_joystick();
}

// --- XInput -----------------------------------------------------------------------

/// `XINPUT_STATE`.
#[repr(C)]
struct XState {
    packet: u32,
    pad: Pad,
}

/// What the call at `0x00B941EB` reaches instead of `XInputGetState`.
unsafe extern "system" fn xinput_get_state(port: u32, state: *mut XState) -> u32 {
    // SAFETY: the stub of the import the call reached before (checked
    // bytes), stdcall.
    let result = unsafe {
        let real: unsafe extern "system" fn(u32, *mut XState) -> u32 =
            std::mem::transmute(addr::XINPUT_GET_STATE as usize);
        real(port, state)
    };
    let port = port as usize;
    if port >= 4 {
        return result;
    }
    let _ = std::panic::catch_unwind(|| {
        let closed = captured();
        with(|s| {
            s.input.serial = s.input.serial.wrapping_add(1);
            if result != 0 || state.is_null() {
                s.input.pads &= !(1 << port);
                s.input.pad[port] = Pad::default();
                return;
            }
            // SAFETY: the state XInput has just written.
            let pad = unsafe { &mut (*state).pad };
            s.input.pads |= 1 << port;
            s.input.pad[port] = *pad;
            if closed {
                *pad = Pad::default();
                return;
            }
            let held = &mut s.held.pad_buttons[port];
            *held &= pad.buttons;
            pad.buttons &= !*held;
            for (value, held) in [&mut pad.left_trigger, &mut pad.right_trigger]
                .into_iter()
                .zip(s.held.pad_triggers[port].iter_mut())
            {
                if *held {
                    if *value <= TRIGGER_HELD {
                        *held = false;
                    } else {
                        *value = 0;
                    }
                }
            }
        });
    });
    result
}

// --- The cursor of the game's menus -----------------------------------------------

/// What the call at `0x0069E6AF` reaches instead of `GetCursorPos`.
unsafe extern "system" fn get_cursor_pos(point: *mut [i32; 2]) -> i32 {
    // SAFETY: the import the call reached before (checked bytes), stdcall.
    let result = unsafe {
        let real: unsafe extern "system" fn(*mut [i32; 2]) -> i32 =
            std::mem::transmute(win::read::<u32>(addr::GET_CURSOR_POS_IMPORT) as usize);
        real(point)
    };
    if result == 0 || point.is_null() {
        return result;
    }
    let _ = std::panic::catch_unwind(|| {
        let closed = captured();
        with(|s| {
            if closed {
                // SAFETY: the point Windows has just written.
                let now = unsafe { *point };
                let kept = *s.cursor.get_or_insert(now);
                // SAFETY: as above.
                unsafe { *point = kept };
            } else {
                s.cursor = None;
            }
        });
    });
    result
}

/// What `0x0069EB00` does, with the movement thrown away while captured:
/// the movement since the last call as floats, both sums set to 0.
unsafe extern "C" fn cursor_delta(out: *mut [f32; 2]) -> u32 {
    // SAFETY: two words in the data of the game, on the main thread that
    // adds to them.
    let (dx, dy) = unsafe {
        let d = (
            win::read::<i32>(addr::CURSOR_DX),
            win::read::<i32>(addr::CURSOR_DY),
        );
        win::write::<i32>(addr::CURSOR_DX, 0);
        win::write::<i32>(addr::CURSOR_DY, 0);
        d
    };
    let (dx, dy) = if captured() { (0, 0) } else { (dx, dy) };
    if !out.is_null() {
        // SAFETY: the game's two floats.
        unsafe { *out = [dx as f32, dy as f32] };
    }
    0
}

// --- Install and the functions of the table ---------------------------------------

/// A jump at `site` to `target`, the rest of `len` bytes filled with `nop`.
///
/// # Safety
/// `site` must be `len` bytes of whole instructions checked by the edition
/// check, patched before the game runs them.
unsafe fn jump(site: u32, len: u32, target: u32) -> bool {
    let mut code = [0x90u8; 8];
    code[0] = 0xE9;
    code[1..5].copy_from_slice(&target.wrapping_sub(site + 5).to_le_bytes());
    // SAFETY: the caller's promise.
    unsafe { win::patch(site, &code[..len as usize]) }
}

/// Patches the four places; from `DllMain`, before the game polls.
pub fn install() -> Result<(), String> {
    let places = [
        (
            addr::INPUT_KEYBOARD,
            addr::INPUT_KEYBOARD_LEN,
            doctor_platform_input_keyboard as unsafe extern "C" fn() as usize as u32,
        ),
        (
            addr::INPUT_MOUSE,
            addr::INPUT_MOUSE_LEN,
            doctor_platform_input_mouse as unsafe extern "C" fn() as usize as u32,
        ),
        (
            addr::INPUT_JOYSTICK,
            addr::INPUT_JOYSTICK_LEN,
            doctor_platform_input_joystick as unsafe extern "C" fn() as usize as u32,
        ),
    ];
    for (site, len, target) in places {
        // SAFETY: checked places, from DllMain.
        if !unsafe { jump(site, len, target) } {
            return Err(format!("could not patch {site:#010x}"));
        }
    }
    let cursor = get_cursor_pos as unsafe extern "system" fn(*mut [i32; 2]) -> i32;
    let mut code = [0x90u8; addr::CURSOR_CALL_LEN as usize];
    code[0] = 0xE8;
    code[1..5].copy_from_slice(
        &(cursor as usize as u32)
            .wrapping_sub(addr::CURSOR_CALL + 5)
            .to_le_bytes(),
    );
    // SAFETY: a checked call through the import (6 bytes), from DllMain;
    // the function has the same convention.
    if !unsafe { win::patch(addr::CURSOR_CALL, &code) } {
        return Err(format!("could not patch {:#010x}", addr::CURSOR_CALL));
    }
    let delta = cursor_delta as unsafe extern "C" fn(*mut [f32; 2]) -> u32;
    // SAFETY: the start of a checked function the module replaces whole.
    if !unsafe {
        jump(
            addr::CURSOR_DELTA,
            addr::CURSOR_DELTA_LEN,
            delta as usize as u32,
        )
    } {
        return Err(format!("could not patch {:#010x}", addr::CURSOR_DELTA));
    }
    let xinput = xinput_get_state as unsafe extern "system" fn(u32, *mut XState) -> u32;
    // SAFETY: a checked call, from DllMain.
    if !unsafe { crate::redirect(addr::XINPUT_CALL, xinput as usize as u32) } {
        return Err(format!("could not patch {:#010x}", addr::XINPUT_CALL));
    }
    Ok(())
}

pub fn read(out: *mut Input) -> i32 {
    if out.is_null() {
        return abi::ERR_ARGUMENT;
    }
    // SAFETY: the mod's buffer; its size is the first word.
    let size = unsafe { (*out).size } as usize;
    if size < std::mem::size_of::<Input>() {
        return abi::ERR_ARGUMENT;
    }
    let input = with(|s| s.input).unwrap_or_default();
    // SAFETY: a buffer at least this large.
    unsafe {
        *out = input;
        (*out).size = std::mem::size_of::<Input>() as u32;
        (*out).captured = CAPTURES.load(Ordering::Relaxed).max(0) as u32;
    }
    abi::OK
}

pub fn capture(on: u32) -> i32 {
    if on != 0 {
        return CAPTURES.fetch_add(1, Ordering::Relaxed) + 1;
    }
    let left = match CAPTURES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        (n > 0).then(|| n - 1)
    }) {
        Ok(before) => before - 1,
        Err(_) => return abi::ERR_STATE,
    };
    if left == 0 {
        // What is held now stays hidden from the game until released.
        with(|s| {
            for (held, key) in s.held.keys.iter_mut().zip(s.input.keys) {
                *held = key & 0x80 != 0;
            }
            for (held, b) in s.held.mouse.iter_mut().zip(s.mouse_buttons) {
                *held = b & 0x80 != 0;
            }
            for (held, b) in s.held.joystick.iter_mut().zip(s.joystick_buttons) {
                *held = b & 0x80 != 0;
            }
            for port in 0..4 {
                let pad = s.input.pad[port];
                s.held.pad_buttons[port] = pad.buttons;
                s.held.pad_triggers[port] = [
                    pad.left_trigger > TRIGGER_HELD,
                    pad.right_trigger > TRIGGER_HELD,
                ];
            }
        });
    }
    left
}

pub fn reserve(key: u32, on: u32) -> i32 {
    let Ok(key) = u8::try_from(key) else {
        return abi::ERR_ARGUMENT;
    };
    with(|s| {
        let count = &mut s.reserved[key as usize];
        if on != 0 {
            *count = count.saturating_add(1);
            abi::OK
        } else if *count > 0 {
            *count -= 1;
            abi::OK
        } else {
            abi::ERR_STATE
        }
    })
    .unwrap_or(abi::ERR_ENGINE)
}
