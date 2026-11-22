//! What the menu does in the game. Called on the main thread, in the frame
//! tick of the platform; the addresses are the ones `doctor_live` uses and
//! the bench has checked (`doctor_toolkit_live::addr`).

use doctor_toolkit_live::addr::{
    ENTITY_MATRIX, FRAME_COMMANDS, LOOP_DISPATCHER, PLAYER_ENTRY_OFFSET, PLAYER_LIST,
    PLAYER_PREPARE_ARG, PLAYER_PREPARE_METHOD, PLAYER_STATE, PLAYER_TELEPORT, TIME_OF_DAY,
    TIME_OF_DAY_HMS, TIME_OF_DAY_SET, TIME_OF_DAY_YEAR, VTABLE_GAME_LOOP,
};
use doctor_toolkit_live::addr::{
    FAMILIES, FAMILIES_COUNT, FAMILIES_LIST, FAMILY_ADD_MONEY, FAMILY_CASH, FAMILY_NAME,
    MONEY_CATEGORY, PLAYER_FAMILY,
};

/// A word of the game's memory; 0 for a null pointer.
///
/// # Safety
/// `address` is 0 or a word the game keeps.
unsafe fn word(address: u32) -> u32 {
    if address < 0x1_0000 {
        return 0;
    }
    // SAFETY: the caller's promise.
    unsafe { *(address as usize as *const u32) }
}

/// The player's family (`corleone`), when the game has the families.
pub fn player_family() -> Option<u32> {
    // SAFETY: the list of families of the game and its entries.
    unsafe {
        let families = word(FAMILIES);
        let list = word(families + FAMILIES_LIST);
        let count = word(families + FAMILIES_COUNT).min(256);
        (0..count)
            .map(|i| word(list + i * 4))
            .find(|&f| f != 0 && word(f + FAMILY_NAME) == PLAYER_FAMILY)
    }
}

/// Books `amount` to the player's family as the game's rewards do (the
/// counter on screen follows); the money after it.
pub fn add_money(amount: f32) -> Option<f32> {
    let family = player_family()?;
    // SAFETY: `AddMoney` of the family (thiscall, callee pops 8), with the
    // arguments of the game's rewards, on the main thread.
    let after = unsafe {
        let add: extern "thiscall" fn(u32, f32, u32) -> f32 =
            std::mem::transmute(FAMILY_ADD_MONEY as usize);
        add(family, amount, MONEY_CATEGORY)
    };
    // SAFETY: the family's money, a float of the family.
    let cash = unsafe { f32::from_bits(word(family + FAMILY_CASH)) };
    Some(if after.is_finite() { after } else { cash })
}

// --- The player (stage 120) ---------------------------------------------------------

/// The damage component of `Player` (`docs/kb/exe/player-damage.md`).
const PLAYER_DAMAGE: u32 = 0x5F4;
const DAMAGE_HEALTH: u32 = 0x10;
const DAMAGE_MAX: u32 = 0x14;
/// Bit 0 — invulnerable (the counter `+0x1C` is above 0).
const DAMAGE_FLAGS: u32 = 0x18;
/// `SetInvulnerable(this, int)`, thiscall, `RET 4`: the counter +1 or −1.
const SET_INVULNERABLE: u32 = 0x009A_E900;
/// Slots of the component's method table: `SetHealth`, `SetMaxHealth`.
const SLOT_SET_HEALTH: u32 = 0x3C;
const SLOT_SET_MAX_HEALTH: u32 = 0x40;
/// The HUD (pointer) and its `Hide`/`Show` (fastcall, a counter).
const HUD: u32 = 0x0112_9C4C;
const HUD_HIDE: u32 = 0x0093_C210;
const HUD_SHOW: u32 = 0x0093_A2F0;
/// Don's view (pointer) and its waypoint: `+0xA4/+0xA8/+0xAC` x, y, z,
/// `+0xB4` set (`docs/kb/exe/don-view.md`).
const DON_VIEW: u32 = 0x0112_AF70;
const WAYPOINT_XYZ: u32 = 0xA4;
const WAYPOINT_SET: u32 = 0xB4;

/// Whether the main loop is the game's (a level), not the menus'.
pub fn in_level() -> bool {
    // SAFETY: the loop dispatcher of the game and its state.
    unsafe { word(word(word(LOOP_DISPATCHER) + 4)) == VTABLE_GAME_LOOP }
}

/// The local player (`Player`), as `PlayerTeleportRelay` finds it.
pub fn player() -> Option<u32> {
    if !in_level() {
        return None;
    }
    // SAFETY: the list of players of the game and its first entry.
    unsafe {
        let list = word(PLAYER_LIST);
        if word(list + 0xC) == 0 {
            return None;
        }
        let entry = word(word(list + 4));
        let player = entry.checked_sub(PLAYER_ENTRY_OFFSET)?;
        let vtable = word(player);
        (0x00D5_5000..0x00E5_0000)
            .contains(&vtable)
            .then_some(player)
    }
}

/// The player's damage component.
pub fn damage() -> Option<u32> {
    // SAFETY: a pointer of the player.
    let d = unsafe { word(player()? + PLAYER_DAMAGE) };
    (d != 0).then_some(d)
}

fn f32_at(address: u32) -> f32 {
    // SAFETY: a float of an object the caller found.
    f32::from_bits(unsafe { word(address) })
}

/// Health and its maximum.
pub fn health(d: u32) -> (f32, f32) {
    (f32_at(d + DAMAGE_HEALTH), f32_at(d + DAMAGE_MAX))
}

pub fn invulnerable(d: u32) -> bool {
    // SAFETY: the flags of the component.
    unsafe { word(d + DAMAGE_FLAGS) & 1 != 0 }
}

/// `SetInvulnerable(on)`: one step of the game's counter.
pub fn set_invulnerable(d: u32, on: bool) {
    // SAFETY: the function of the game the class SetInvulnerability calls,
    // on the main thread.
    unsafe {
        let f: extern "thiscall" fn(u32, i32) = std::mem::transmute(SET_INVULNERABLE as usize);
        f(d, i32::from(on));
    }
}

/// A float setter of the component by its slot (`SetHealth`,
/// `SetMaxHealth`): the game's listeners follow.
fn call_slot(d: u32, slot: u32, value: f32) {
    // SAFETY: the method table of the component (in the image) and its
    // thiscall setters, on the main thread.
    unsafe {
        let table = word(d);
        let f = word(table + slot);
        if !(0x0040_0000..0x00D5_0000).contains(&f) {
            return;
        }
        let f: extern "thiscall" fn(u32, f32) = std::mem::transmute(f as usize);
        f(d, value);
    }
}

pub fn set_health(d: u32, value: f32) {
    call_slot(d, SLOT_SET_HEALTH, value);
}

pub fn set_max_health(d: u32, value: f32) {
    call_slot(d, SLOT_SET_MAX_HEALTH, value);
}

/// The HUD object, when there is one.
pub fn hud() -> Option<u32> {
    // SAFETY: a pointer in the data of the game.
    let h = unsafe { word(HUD) };
    (h != 0).then_some(h)
}

/// One step of the HUD's counter: hide (`true`) or show.
pub fn hud_hide(h: u32, hide: bool) {
    // SAFETY: `Hud::Hide`/`Hud::Show` of the game (fastcall, the HUD in
    // ECX), on the main thread.
    unsafe {
        let f: extern "fastcall" fn(u32) =
            std::mem::transmute(if hide { HUD_HIDE } else { HUD_SHOW } as usize);
        f(h);
    }
}

/// The player's position (the translation of the world matrix).
pub fn position() -> Option<[f32; 3]> {
    let p = player()?;
    let m = p + ENTITY_MATRIX + 0x30;
    Some([f32_at(m), f32_at(m + 4), f32_at(m + 8)])
}

/// The waypoint the player put in Don's view, while the view exists.
pub fn waypoint() -> Option<[f32; 3]> {
    // SAFETY: Don's view of the game and its fields.
    unsafe {
        let v = word(DON_VIEW);
        if v == 0 || *((v + WAYPOINT_SET) as usize as *const u8) == 0 {
            return None;
        }
        let at = v + WAYPOINT_XYZ;
        let xyz = [f32_at(at), f32_at(at + 4), f32_at(at + 8)];
        xyz.iter()
            .all(|c| c.is_finite() && c.abs() < 1.0e5)
            .then_some(xyz)
    }
}

/// Teleports the player as `PlayerTeleportRelay` does (the calls of
/// `0x007DD062` and `0x007DD117`, as `doctor_live`): `Err` says why not.
pub fn teleport(to: [f32; 3]) -> Result<(), &'static str> {
    let p = player().ok_or("no player")?;
    // SAFETY: fields and methods of the player, the frame buffer of the
    // game; on the main thread inside the frame (the tick).
    unsafe {
        if word(FRAME_COMMANDS) == 0 {
            return Err("no frame");
        }
        if word(p + PLAYER_STATE) >> 7 & 1 != 0 {
            return Err("the game does not move the player now");
        }
        let m = p + ENTITY_MATRIX;
        let mut matrix = [0f32; 16];
        for (i, v) in matrix.iter_mut().enumerate() {
            *v = f32_at(m + i as u32 * 4);
        }
        matrix[12] = to[0];
        matrix[13] = to[1];
        matrix[14] = to[2];
        let prepare = word(word(p) + PLAYER_PREPARE_METHOD);
        if !(0x0040_0000..0x00D5_0000).contains(&prepare) {
            return Err("no method");
        }
        let done = [0u32; 2];
        let partitions = [0u32; 5];
        let prepare: extern "thiscall" fn(u32, u32, u32, u32) =
            std::mem::transmute(prepare as usize);
        prepare(p, PLAYER_PREPARE_ARG, 1, 0);
        let teleport: extern "thiscall" fn(u32, *const [f32; 16], u32, *const u32, *const u32) =
            std::mem::transmute(PLAYER_TELEPORT as usize);
        teleport(p, &matrix, 0, done.as_ptr(), partitions.as_ptr());
    }
    Ok(())
}

/// The hour of the game's clock.
pub fn hour() -> Option<u32> {
    // SAFETY: the TimeOfDayManager of the level.
    unsafe {
        let tod = word(TIME_OF_DAY);
        (tod != 0).then(|| word(tod + TIME_OF_DAY_HMS))
    }
}

/// Sets the clock to `hour:00:00` as `SetTimeOfDay` does.
pub fn set_hour(hour: u32) -> bool {
    // SAFETY: `SetTime` of the TimeOfDayManager with the year and the day
    // it has, on the main thread (as doctor_live).
    unsafe {
        let tod = word(TIME_OF_DAY);
        if tod == 0 {
            return false;
        }
        let (year, day) = (
            word(tod + TIME_OF_DAY_YEAR),
            word(tod + TIME_OF_DAY_YEAR + 4),
        );
        let set: extern "thiscall" fn(u32, u32, u32, u32, u32, u32) =
            std::mem::transmute(TIME_OF_DAY_SET as usize);
        set(tod, year, day, hour.min(23), 0, 0);
    }
    true
}

/// The game's pause: counters by bit (`0x01205260`, 32 words); `Pause(mask)`
/// adds one to the counters of the bits and stops the world (tick mode 1),
/// `Resume(mask)` takes one away and lets the world go when every counter
/// is 0 (`docs/kb/exe/main-loop.md`). The game uses bits 1 (its pause
/// menu, hints, the window losing focus) and 8; the mod takes 0x10.
const PAUSE: u32 = 0x0040_EAC0;
const RESUME: u32 = 0x0040_EAF0;
const PAUSE_BIT: u32 = 0x10;

/// Stops (`true`) or lets go the world for the menu.
pub fn pause(on: bool) {
    // SAFETY: the game's pause functions (cdecl, a mask), on the main
    // thread.
    unsafe {
        let f: extern "C" fn(u32) = std::mem::transmute(if on { PAUSE } else { RESUME } as usize);
        f(PAUSE_BIT);
    }
}

// --- Weapons (stage 121) ------------------------------------------------------------
//
// `docs/kb/exe/weapons-gun.md`: a `Gun` keeps base values (attributes and
// level) at `+0x384…+0x39F` and the effective copies the shooting reads at
// `+0x46C…+0x487`; the game copies base to effective on every switch and
// pick-up (`0x007014D0`).

/// The handle of the item in the player's hands (`− 0x48` is the item), and
/// the type byte that is 1 for a `Gun` (`0x00800A90`), and bit 10 of the
/// player's word `+0x8E0`: the weapon is in the hands, not holstered
/// (`0x007AB1F0`).
const PLAYER_ITEM: u32 = 0x74C;
const ITEM_KIND: u32 = 0x1A8;
const PLAYER_HANDS: u32 = 0x8E0;
const HANDS_ARMED: u32 = 1 << 10;
const GUN_FLAGS: u32 = 0x2D8;
/// Bit 5 — a reload takes nothing from the store; bit 6 — "infinite
/// ammunition" (bit 5 follows it on every reset).
pub const GUN_INFINITE: u32 = 0x60;
/// Base damage, interval between shots (s), the two ends of the spread.
pub const GUN_BASE: [u32; 4] = [0x38C, 0x390, 0x394, 0x398];
/// Their effective copies.
pub const GUN_EFFECTIVE: [u32; 4] = [0x474, 0x478, 0x47C, 0x480];
/// The camera at a shot (shake, kick; attributes 108, 109).
pub const GUN_CAMERA: [u32; 2] = [0x4A4, 0x4A8];

/// The `Gun` in the player's hands (not one holstered).
pub fn gun() -> Option<u32> {
    let p = player()?;
    // SAFETY: fields of the player and of its item.
    unsafe {
        if word(p + PLAYER_HANDS) & HANDS_ARMED == 0 {
            return None;
        }
        let item = word(p + PLAYER_ITEM).checked_sub(0x48)?;
        // One byte, as the game compares it (`0x00800AB2`); the bytes after
        // it are other fields (`01 02 02 00` for a Tommy gun).
        (item >= 0x1_0000 && word(item + ITEM_KIND) & 0xFF == 1).then_some(item)
    }
}

pub fn gun_flags(g: u32) -> u32 {
    // SAFETY: a word of the gun.
    unsafe { word(g + GUN_FLAGS) }
}

pub fn set_gun_flags(g: u32, flags: u32) {
    // SAFETY: a word of the gun, written by the game the same way.
    unsafe { std::ptr::write_volatile((g + GUN_FLAGS) as usize as *mut u32, flags) };
}

pub fn gun_f32(g: u32, offset: u32) -> f32 {
    f32_at(g + offset)
}

pub fn set_gun_f32(g: u32, offset: u32, value: f32) {
    // SAFETY: a float of the gun, written by the game the same way.
    unsafe { std::ptr::write_volatile((g + offset) as usize as *mut f32, value) };
}

/// The player's object handle as `DamageInfo+4` names the one who hit.
pub fn player_handle() -> Option<u32> {
    player().map(|p| p + 0x48)
}

/// The places the mod hooks, with the bytes it expects there.
pub const NO_RELOAD_AT: u32 = 0x0070_334A;
/// `add dword ptr [esi+0x328], -1` in `Gun::Fire`: the magazine −1.
pub const NO_RELOAD_BYTES: [u8; 7] = [0x83, 0x86, 0x28, 0x03, 0x00, 0x00, 0xFF];
/// Where `Gun::Fire` goes on after it.
pub const NO_RELOAD_RESUME: u32 = 0x0070_3351;
pub const ENEMY_DAMAGE_AT: u32 = 0x0079_D1E2;
/// `call 0x008001E0` (`Character::OnDamage(DamageInfo*)`, thiscall) at the
/// end of `Player::OnDamage`.
pub const ENEMY_DAMAGE_BYTES: [u8; 5] = [0xE8, 0xF9, 0x2F, 0x06, 0x00];
/// The damage of a `DamageInfo` (f32) and the handle of who hit.
pub const DAMAGE_AMOUNT: u32 = 0x2C;
pub const DAMAGE_BY: u32 = 0x04;

/// Whether the code at `address` is `bytes`.
pub fn code_is(address: u32, bytes: &[u8]) -> bool {
    // SAFETY: code of the game's image.
    let here = unsafe { std::slice::from_raw_parts(address as usize as *const u8, bytes.len()) };
    here == bytes
}
