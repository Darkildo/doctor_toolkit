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

/// The game's physics: a pointer whose `+0x24` is the Havok world
/// (`hkpWorld`), as the character proxy (`0x00550CD1`) and the ground snap
/// of the game (`0x007080F0`) read it.
const PHYSICS: u32 = 0x0122_3480;
const PHYSICS_WORLD: u32 = 0x24;
/// The game's builder of a ray query (thiscall on the input, `RET 0x18`):
/// from, to, then the filter of the game (`2, 0x80000000, 0, 0` is the
/// ground the game snaps points to).
const RAY_INPUT: u32 = 0x0054_25D0;
const RAY_INPUT_CODE: [u8; 6] = [0x8B, 0x54, 0x24, 0x0C, 0x8B, 0xC1];
/// `hkpWorld::castRay(input, output)`, thiscall on the world, `RET 8`.
const CAST_RAY: u32 = 0x009E_69D0;
const CAST_RAY_CODE: [u8; 9] = [0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF0, 0x83, 0xEC, 0x40];
/// The output: the fraction of the way at `+0x10`, the body hit at `+0x50`.
const RAY_FRACTION: usize = 0x10 / 4;
const RAY_HIT: usize = 0x50 / 4;

/// A ray query or its output, aligned as Havok's vectors are.
#[repr(C, align(16))]
struct Ray([u32; 32]);

/// Whether the game's ray query is the one this module calls.
pub fn ray_ready() -> bool {
    code_is(RAY_INPUT, &RAY_INPUT_CODE) && code_is(CAST_RAY, &CAST_RAY_CODE)
}

/// The height of the first ground the game's physics has under `x, z`,
/// going down from `top` to `bottom`: a roof, a street or a hill, as the
/// game snaps points to the ground (`0x007080F0`). `None` where the physics
/// has nothing loaded there yet.
pub fn ground(x: f32, z: f32, top: f32, bottom: f32) -> Option<f32> {
    if !ray_ready() || top <= bottom {
        return None;
    }
    // SAFETY: the game's physics, checked for null; its builder of a ray
    // query and `castRay` (checked above), with buffers larger than the
    // game's own (0x5C and 0x60 bytes) and aligned as the game aligns them;
    // on the main thread inside the frame (the tick), as the game queries.
    unsafe {
        let world = word(word(PHYSICS) + PHYSICS_WORLD);
        if world == 0 {
            return None;
        }
        let from = [x, top, z, 0.0f32];
        let to = [x, bottom, z, 0.0f32];
        let mut input = Ray([0; 32]);
        let build: extern "thiscall" fn(
            *mut Ray,
            *const [f32; 4],
            *const [f32; 4],
            u32,
            u32,
            u32,
            u32,
        ) = std::mem::transmute(RAY_INPUT as usize);
        build(&mut input, &from, &to, 2, 0x8000_0000, 0, 0);
        let mut output = Ray([0; 32]);
        output.0[RAY_FRACTION] = 1.0f32.to_bits();
        output.0[RAY_FRACTION + 1] = u32::MAX;
        output.0[0x20 / 4] = u32::MAX;
        let cast: extern "thiscall" fn(u32, *const Ray, *mut Ray) =
            std::mem::transmute(CAST_RAY as usize);
        cast(world, &input, &mut output);
        if output.0[RAY_HIT] == 0 {
            return None;
        }
        let fraction = f32::from_bits(output.0[RAY_FRACTION]);
        (0.0..=1.0)
            .contains(&fraction)
            .then_some(top - fraction * (top - bottom))
    }
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

/// The player's crew (pointer, `doctor_toolkit_live::addr::CREW`): the
/// level of the tree `+0x14`, seven places of 0x24 bytes from `+0x18`.
const CREW: u32 = doctor_toolkit_live::addr::CREW;
const CREW_LEVEL: u32 = 0x14;
const CREW_PLACES: u32 = 0x18;
const PLACE_SIZE: u32 = 0x24;
pub const PLACES: usize = 7;
/// In a place: flags (bit 0 open, bit 1 taken), rank, the member's handle
/// (`SimNpc + 0x48`), specialties.
const PLACE_RANK: u32 = 4;
const PLACE_MEMBER: u32 = 8;
const PLACE_SPECIALTIES: u32 = 0x10;
const HANDLE_OFFSET: u32 = 0x48;
/// The level of the tree that opens each place (`0x0090B480`).
const PLACE_LEVEL: [u32; PLACES] = [1, 2, 3, 5, 4, 7, 8];
/// Opens the places of the tree's level (fastcall on the crew).
const OPEN_PLACES: u32 = 0x0090_B480;
const OPEN_PLACES_CODE: [u8; 8] = [0x8B, 0x41, 0x14, 0x83, 0xC0, 0xFF, 0x83, 0xF8];
/// A member: his GUID (`+0x20`, 16 bytes) and the hash of his name's text
/// (`+0xC4`, the key of the game's string tables; `0x009263E8`).
const MEMBER_GUID: u32 = 0x20;
const MEMBER_NAME: u32 = 0xC4;
/// Merits and licences (pointer): `+8` entries of 0x2C bytes, `+0xC` their
/// number; in an entry the member's GUID `+0x18`, the licence `+0x28` (1–4).
const HONORS: u32 = 0x0112_9908;
const HONOR_SIZE: u32 = 0x2C;
const HONOR_GUID: u32 = 0x18;
const HONOR_LICENCE: u32 = 0x28;
/// The game's strings (pointer, `0x0097F8E0`): `+0x30` tables, `+0x34` their
/// number, searched from the last (`0x006039D0`); an entry has its table at
/// `+4` and "UTF-16" at `+8`; a table has the number of strings `+0x10`, the
/// sorted hashes `+0x18`, the strings `+0x1C` (`0x00605D00`).
const STRINGS: u32 = 0x0122_34A4;
/// The escort component (packet `0xE851E4F3`): its index among the
/// components of an object (`0x0068A980`), its method table; the number of
/// men the player may take at `+0x34` (attribute 1, 3 by default).
const ESCORT_INDEX: u32 = 0x0113_0FA0;
const ESCORT_VTABLE: u32 = 0x00D9_31A8;
const ESCORT_COUNT: u32 = 0x34;

/// A place of the player's crew.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Place {
    pub open: bool,
    pub taken: bool,
    /// 3 soldier, 4 capo, 5 underboss.
    pub rank: u32,
    /// The member (`SimNpc`) when the place is taken.
    pub member: Option<u32>,
    pub specialties: u32,
}

fn place_address(i: usize) -> Option<u32> {
    if i >= PLACES || !in_level() {
        return None;
    }
    // SAFETY: the crew pointer of the game.
    let crew = unsafe { word(CREW) };
    (crew != 0).then_some(crew + CREW_PLACES + i as u32 * PLACE_SIZE)
}

/// Place `i` (0–6) of the crew.
pub fn place(i: usize) -> Option<Place> {
    let at = place_address(i)?;
    // SAFETY: the place of the crew and its member.
    unsafe {
        let flags = word(at);
        let taken = flags & 2 != 0;
        let handle = word(at + PLACE_MEMBER);
        let member = (taken && handle > HANDLE_OFFSET + 0x1_0000).then(|| handle - HANDLE_OFFSET);
        Some(Place {
            open: flags & 1 != 0,
            taken,
            rank: word(at + PLACE_RANK),
            member,
            specialties: word(at + PLACE_SPECIALTIES),
        })
    }
}

/// Writes the specialties of a taken place; the member in the world takes
/// them when he is made again (`0x0090A5C5`).
pub fn set_specialties(i: usize, bits: u32) -> bool {
    let Some(at) = place_address(i) else {
        return false;
    };
    // SAFETY: the place of the crew.
    unsafe {
        if word(at) & 2 == 0 {
            return false;
        }
        let old = word(at + PLACE_SPECIALTIES);
        *((at + PLACE_SPECIALTIES) as usize as *mut u32) = (old & !0xFC) | (bits & 0xFC);
    }
    true
}

/// Opens place `i` as the game does: the level of the tree goes up to the
/// one that opens it (it never goes down and goes into the save), then the
/// game opens the places of that level.
pub fn open_place(i: usize) -> bool {
    if i >= PLACES || !code_is(OPEN_PLACES, &OPEN_PLACES_CODE) {
        return false;
    }
    // SAFETY: the crew of the game and its function that opens the places of
    // the level (fastcall, the crew in ECX); on the main thread.
    unsafe {
        let crew = word(CREW);
        if crew == 0 || !in_level() {
            return false;
        }
        let level = (crew + CREW_LEVEL) as usize as *mut u32;
        if *level < PLACE_LEVEL[i] {
            *level = PLACE_LEVEL[i];
        }
        let open: extern "fastcall" fn(u32) = std::mem::transmute(OPEN_PLACES as usize);
        open(crew);
        word(crew + CREW_PLACES + i as u32 * PLACE_SIZE) & 1 != 0
    }
}

/// The text of the game's strings with this hash, in the game's language.
pub fn game_text(hash: u32) -> Option<String> {
    // SAFETY: the string tables of the game, read only.
    unsafe {
        let strings = word(STRINGS);
        let tables = word(strings + 0x30);
        let count = word(strings + 0x34);
        if tables == 0 || count > 64 {
            return None;
        }
        for t in (0..count).rev() {
            let entry = word(tables + t * 4);
            let table = word(entry + 4);
            let n = word(table + 0x10);
            let hashes = word(table + 0x18);
            if table == 0 || hashes == 0 || n > 1_000_000 {
                continue;
            }
            let (mut lo, mut hi) = (0u32, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let h = word(hashes + mid * 4);
                if h == hash {
                    let text = word(word(table + 0x1C) + mid * 4);
                    let wide = *((entry + 8) as usize as *const u8) != 0;
                    return read_text(text, wide);
                }
                if h < hash {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
        }
        None
    }
}

/// A zero-terminated string of the game, UTF-8 or UTF-16, at most 120
/// characters.
///
/// # Safety
/// `at` is 0 or a string the game keeps.
unsafe fn read_text(at: u32, wide: bool) -> Option<String> {
    if at < 0x1_0000 {
        return None;
    }
    // SAFETY: the caller's promise; reading stops at the terminator.
    unsafe {
        if wide {
            let p = at as usize as *const u16;
            let len = (0..120).find(|&k| *p.add(k) == 0)?;
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
        } else {
            let p = at as usize as *const u8;
            let len = (0..240).find(|&k| *p.add(k) == 0)?;
            Some(String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned())
        }
    }
}

/// The name of a member, from the game's strings.
pub fn member_name(member: u32) -> Option<String> {
    // SAFETY: the member's field.
    let hash = unsafe { word(member + MEMBER_NAME) };
    game_text(hash).filter(|t| !t.trim().is_empty())
}

/// The licence byte of a member in the game's merits (1–4).
fn licence_address(member: u32) -> Option<u32> {
    // SAFETY: the member's GUID and the merits of the game, read only.
    unsafe {
        let guid = std::slice::from_raw_parts((member + MEMBER_GUID) as usize as *const u8, 16);
        let honors = word(HONORS);
        let entries = word(honors + 8);
        let count = word(honors + 0xC);
        if entries == 0 || count > 4096 {
            return None;
        }
        (0..count)
            .map(|k| entries + k * HONOR_SIZE)
            .find(|&e| {
                std::slice::from_raw_parts((e + HONOR_GUID) as usize as *const u8, 16) == guid
            })
            .map(|e| e + HONOR_LICENCE)
    }
}

/// The weapon licence of a member (1–4); 1 when the game has no record.
pub fn licence(member: u32) -> u32 {
    // SAFETY: the licence byte of the record.
    licence_address(member).map_or(1, |a| unsafe { *(a as usize as *const u8) } as u32)
}

/// Sets the weapon licence of a member who has a record.
pub fn set_licence(member: u32, level: u32) -> bool {
    let Some(at) = licence_address(member) else {
        return false;
    };
    // SAFETY: the licence byte of the record.
    unsafe { *(at as usize as *mut u8) = level.clamp(1, 4) as u8 };
    true
}

/// The player's escort component (checked by its method table).
pub fn escort() -> Option<u32> {
    let p = player()?;
    // SAFETY: the component table of the player (`0x0043B870`): `+0x38`
    // components, `+0x34` the indices of the kinds.
    unsafe {
        let index = word(ESCORT_INDEX);
        let components = word(p + 0x38);
        let indices = word(p + 0x34);
        if components == 0 || indices == 0 || index > 0x400 {
            return None;
        }
        let slot = *((indices + 0xE + index * 2) as usize as *const u16) as u32;
        let c = word(components + slot * 4);
        (c != 0 && word(c) == ESCORT_VTABLE).then_some(c)
    }
}

/// How many men the player may take with him.
pub fn escort_count(c: u32) -> u32 {
    // SAFETY: the field of the escort component.
    unsafe { word(c + ESCORT_COUNT) }
}

pub fn set_escort_count(c: u32, n: u32) {
    // SAFETY: the field of the escort component.
    unsafe { *((c + ESCORT_COUNT) as usize as *mut u32) = n }
}
