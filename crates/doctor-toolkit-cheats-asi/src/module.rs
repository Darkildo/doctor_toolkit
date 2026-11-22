//! The module in the game: `doctor_mod_init`, the tick, the overlay.

use std::collections::HashMap;
use std::ffi::{CString, c_void};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;

use doctor_overlay::Painter;
use doctor_sdk::Platform;
use doctor_sdk::abi::{self, Api, Input};

use crate::config::{self, Config};
use crate::menu::{Effect, Key, Menu, Repeat, What};
use crate::text::{self, Lang, T};
use crate::{VERSION, game, hooks, view};

struct State {
    config: Config,
    lang: Lang,
    menu: Menu,
    open: bool,
    repeat: Repeat,
    /// The opening key or chord was down at the last tick.
    opener_down: bool,
    /// A line under the menu and until when (ms) it shows.
    status: Option<(String, u32)>,
    started: Instant,
    keep: Keep,
    /// The player's position, for the item Coordinates.
    coords: Option<[f32; 3]>,
    /// The chosen place of the crew as the menu shows it, and the member
    /// whose name it holds.
    place_label: Option<String>,
    place_name: Option<(u32, Option<String>)>,
    /// `doctor_cheats.cfg` next to the module.
    config_path: Option<PathBuf>,
}

/// What the toggles of the menu hold in the game, from tick to tick.
#[derive(Default)]
struct Keep {
    /// God mode: the damage component and the steps of its counter the mod
    /// added.
    god: Option<(u32, u32)>,
    /// One hit: the component and the maximum of health it had.
    one_hit: Option<(u32, f32)>,
    /// The HUD the mod has hidden.
    hud: Option<u32>,
    /// A teleport waiting for the game to go on, then for the ground.
    teleport: Option<Teleport>,
    /// The mod holds the game's pause while the menu is open.
    paused: bool,
    /// Guns the mod gave infinite ammunition, with the bits they had.
    ammo: HashMap<u32, u32>,
    /// Guns the mod scales: their own damage, interval and spread, and
    /// what the mod wrote.
    scale: HashMap<u32, ([f32; 4], [f32; 4])>,
    /// Guns whose camera at a shot the mod has calmed, with what it was.
    camera: HashMap<u32, [f32; 2]>,
}

/// A teleport to the waypoint: its `x, z`, where the player was, when it
/// began, and its step.
#[derive(Clone, Copy)]
struct Teleport {
    x: f32,
    z: f32,
    from: Option<[f32; 3]>,
    started: Option<Instant>,
    step: Step,
}

#[derive(Clone, Copy)]
enum Step {
    /// Looking for the ground under the waypoint.
    Find,
    /// The player hangs above the waypoint (since then) while the game
    /// loads the ground there.
    Hang(Instant),
    /// The player goes here (nowhere for NaN), then the menu says this; the
    /// game refuses while its own teleport runs (bench §69), so it is tried
    /// again on the next tick.
    Move([f32; 3], T),
}

/// The ray for the ground under the waypoint goes from this height down to
/// the next; the player waits for the ground at least this high.
const RAY_TOP: f32 = 400.0;
const RAY_BOTTOM: f32 = -100.0;
const HANG: f32 = 40.0;
/// How long the player waits above the waypoint, and how long the whole
/// teleport may take, in seconds.
const HANG_SECONDS: f32 = 15.0;
const TELEPORT_SECONDS: f32 = 25.0;
/// Above the ground the feet go, as the game's ground snap (`0x007080F0`).
const ABOVE_GROUND: f32 = 0.1;
/// The ray does not see water: ground below this is the sea floor, where
/// the player drowns (bench §69).
const SEA_FLOOR: f32 = -0.5;

/// The mode of the frame tick in which the world moves.
const RUNNING: u32 = abi::TICK_RUNNING;

static STATE: Mutex<Option<State>> = Mutex::new(None);
static PAINTER: Mutex<Painter> = Mutex::new(Painter::new());
static OVERLAY: AtomicI32 = AtomicI32::new(0);

/// The licences of the two fonts the module carries (SIL Open Font
/// License: the licence goes with the font, here inside the module next to
/// the unmodified font files).
#[used]
static FONT_LICENCES: [(&str, &str); 2] = doctor_overlay::FONT_LICENCES;

const STATUS_MS: u32 = 3500;
/// XInput buttons.
const PAD_UP: u16 = 0x0001;
const PAD_DOWN: u16 = 0x0002;
const PAD_LEFT: u16 = 0x0004;
const PAD_RIGHT: u16 = 0x0008;
const PAD_BACK: u16 = 0x0020;
const PAD_RB: u16 = 0x0200;
const PAD_A: u16 = 0x1000;
const PAD_B: u16 = 0x2000;
/// DirectInput scan codes the menu reads.
const KEY_BACKSPACE: u8 = 0x0E;
const KEY_NUMPAD_ENTER: u8 = 0x9C;

fn log(p: &Platform, text: &str) {
    p.log(&CString::new(text).unwrap_or_default());
}

/// A line in the platform's log, when the platform is there.
fn note(text: &str) {
    if let Some(p) = Platform::get() {
        log(&p, text);
    }
}

unsafe extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueA(
        key: usize,
        sub_key: *const u8,
        value: *const u8,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

/// The folder of this module (`scripts/` of the game).
fn module_dir() -> Option<PathBuf> {
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    let mut module = std::ptr::null_mut();
    let mut name = [0u16; 1024];
    // SAFETY: plain Win32 calls with buffers of their size.
    let n = unsafe {
        if GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            module_dir as fn() -> Option<PathBuf> as usize as *const u16,
            &mut module,
        ) == 0
        {
            return None;
        }
        GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32)
    };
    let path = PathBuf::from(String::from_utf16_lossy(&name[..n as usize]));
    path.parent().map(PathBuf::from)
}

/// `Locale` of `HKLM\SOFTWARE\EA Games\The Godfather II` (32-bit view).
fn registry_locale() -> Option<String> {
    const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
    const RRF_RT_REG_SZ: u32 = 0x2;
    let mut buf = [0u8; 32];
    let mut size = buf.len() as u32;
    // SAFETY: a plain Win32 call with a buffer of its size.
    let r = unsafe {
        RegGetValueA(
            HKEY_LOCAL_MACHINE,
            c"SOFTWARE\\EA Games\\The Godfather II".as_ptr().cast(),
            c"Locale".as_ptr().cast(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != 0 {
        return None;
    }
    let text = buf.split(|&b| b == 0).next().unwrap_or(&[]);
    Some(String::from_utf8_lossy(text).into_owned())
}

fn ms(state: &State) -> u32 {
    state.started.elapsed().as_millis() as u32
}

fn set_status(state: &mut State, text: String) {
    let until = ms(state).wrapping_add(STATUS_MS);
    state.status = Some((text, until));
}

fn open_menu(p: &Platform, state: &mut State, open: bool) {
    if state.open == open {
        return;
    }
    state.open = open;
    if open {
        state.menu.reset();
        state.status = None;
    }
    // The world stands while the menu is open (a level only).
    if open && game::in_level() && !state.keep.paused {
        game::pause(true);
        state.keep.paused = true;
    } else if !open && state.keep.paused {
        game::pause(false);
        state.keep.paused = false;
    }
    if !open && state.menu.on(What::Remember) {
        remember(state);
    }
    p.overlay_show(OVERLAY.load(Ordering::Relaxed), open);
    let captures = p.capture_input(open);
    log(
        p,
        &format!(
            "doctor_cheats: menu {}, captures held {captures}",
            if open { "open" } else { "closed" }
        ),
    );
}

/// The keys of the menu held now, from the keyboard and the pads.
fn held(input: &Input, pad: bool) -> Vec<Key> {
    let key = |k: u8| input.keys[k as usize] & 0x80 != 0;
    let mut buttons = 0u16;
    if pad {
        for i in 0..4 {
            if input.pads & (1 << i) != 0 {
                buttons |= input.pad[i].buttons;
            }
        }
    }
    let mut out = Vec::new();
    for (down, k) in [
        (key(abi::KEY_UP) || buttons & PAD_UP != 0, Key::Up),
        (key(abi::KEY_DOWN) || buttons & PAD_DOWN != 0, Key::Down),
        (key(abi::KEY_LEFT) || buttons & PAD_LEFT != 0, Key::Left),
        (key(abi::KEY_RIGHT) || buttons & PAD_RIGHT != 0, Key::Right),
        (
            key(abi::KEY_RETURN) || key(KEY_NUMPAD_ENTER) || buttons & PAD_A != 0,
            Key::Enter,
        ),
        (
            key(abi::KEY_ESCAPE) || key(KEY_BACKSPACE) || buttons & PAD_B != 0,
            Key::Back,
        ),
    ] {
        if down {
            out.push(k);
        }
    }
    out
}

/// The opening key, or Back + RB on any pad.
fn opener(input: &Input, config: &Config) -> bool {
    if input.keys[config.key as usize] & 0x80 != 0 {
        return true;
    }
    config.pad
        && (0..4).any(|i| {
            input.pads & (1 << i) != 0
                && input.pad[i].buttons & (PAD_BACK | PAD_RB) == PAD_BACK | PAD_RB
        })
}

/// An action of the menu, on the main thread.
fn run(state: &mut State, what: What) {
    let lang = state.lang;
    let text = match what {
        What::Money => match game::add_money(10_000.0) {
            Some(after) => format!(
                "{}{}",
                T::MoneyDone.get(lang),
                view::money(after.round() as i64, lang)
            ),
            None => T::NotHere.get(lang).to_owned(),
        },
        What::CrewSlot => {
            let i = chosen_place(state);
            match game::place(i) {
                None => T::NotHere,
                Some(p) if p.open => T::PlaceWasOpen,
                Some(_) if game::open_place(i) => T::PlaceOpened,
                Some(_) => T::NotHere,
            }
            .get(lang)
            .to_owned()
        }
        What::Teleport => match game::waypoint() {
            None if game::player().is_none() => T::NotHere.get(lang).to_owned(),
            None => T::NoWaypoint.get(lang).to_owned(),
            Some(to) => {
                // The height of the waypoint is always 0: the game's physics
                // gives the ground under it (stage 120b).
                state.keep.teleport = Some(Teleport {
                    x: to[0],
                    z: to[2],
                    from: game::position(),
                    started: None,
                    step: Step::Find,
                });
                T::TeleportLater.get(lang).to_owned()
            }
        },
        _ => T::NotYet.get(lang).to_owned(),
    };
    set_status(state, text);
}

/// A value of the menu changed.
/// Writes the settings and, with "Remember the settings" on, the values of
/// the menu into `doctor_cheats.cfg`; the result goes to the status line.
fn remember(state: &mut State) {
    let Some(path) = state.config_path.clone() else {
        return;
    };
    state.config.remember = state.menu.on(What::Remember);
    state.config.values = state.menu.values();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let text = match std::fs::write(&path, state.config.write(&old)) {
        Ok(()) => T::Remembered,
        Err(_) => T::NotRemembered,
    };
    let text = text.get(state.lang).to_owned();
    set_status(state, text);
}

fn changed(state: &mut State, what: What) {
    let lang = state.lang;
    if what == What::Remember {
        remember(state);
        return;
    }
    if matches!(
        what,
        What::CrewPlace | What::Specialty(_) | What::CrewLicence | What::CrewEscort
    ) {
        crew_changed(state, what);
        return;
    }
    if what == What::Hour {
        let hour = state.menu.value(What::Hour).unwrap_or(12.0) as u32;
        let text = if game::set_hour(hour) {
            format!("{}{hour:02}:00", T::HourSet.get(lang))
        } else {
            T::NotHere.get(lang).to_owned()
        };
        set_status(state, text);
    }
}

/// The specialties of the crew, as the items of the page.
const SPECIALTIES: [u32; 6] = [0x04, 0x08, 0x10, 0x20, 0x40, 0x80];

/// The place (0–6) chosen on the Crew page.
fn chosen_place(state: &State) -> usize {
    (state.menu.value(What::CrewPlace).unwrap_or(1.0) as usize).clamp(1, game::PLACES) - 1
}

/// The Crew page shows what the game has for the chosen place: its rank
/// and member, his specialties and licence, and the escort of the player.
fn crew(state: &mut State) {
    let lang = state.lang;
    // Nothing is read from the game while the page is not ready.
    if !state.menu.item(What::CrewPlace).is_some_and(|i| i.ready) {
        state.place_label = None;
        return;
    }
    let i = chosen_place(state);
    let Some(place) = game::place(i) else {
        state.place_label = None;
        return;
    };
    let rank = match place.rank {
        3 => T::Soldier.get(lang),
        4 => T::Capo.get(lang),
        5 => T::Underboss.get(lang),
        _ => "-",
    };
    let who = match place.member {
        Some(m) => {
            if state.place_name.as_ref().map(|(k, _)| *k) != Some(m) {
                state.place_name = Some((m, game::member_name(m)));
            }
            state
                .place_name
                .as_ref()
                .and_then(|(_, n)| n.clone())
                .unwrap_or_else(|| "?".to_owned())
        }
        None if place.open => T::PlaceFree.get(lang).to_owned(),
        None => T::PlaceClosed.get(lang).to_owned(),
    };
    state.place_label = Some(format!("{}: {rank}, {who}", i + 1));
    for bit in SPECIALTIES {
        let on = place.member.is_some() && place.specialties & bit != 0;
        state.menu.set_toggle(What::Specialty(bit), on);
    }
    if let Some(m) = place.member {
        state
            .menu
            .set_value(What::CrewLicence, game::licence(m) as f32);
    }
    if let Some(c) = game::escort() {
        state
            .menu
            .set_value(What::CrewEscort, game::escort_count(c) as f32);
    }
}

/// A change on the Crew page goes into the game; what the game refuses is
/// shown back by [`crew`] on the next tick.
fn crew_changed(state: &mut State, what: What) {
    let lang = state.lang;
    let i = chosen_place(state);
    let member = game::place(i).and_then(|p| p.member);
    let text = match what {
        What::CrewPlace => {
            state.place_name = None;
            return;
        }
        What::Specialty(_) => {
            let bits = SPECIALTIES
                .iter()
                .filter(|&&b| state.menu.on(What::Specialty(b)))
                .fold(0, |acc, b| acc | b);
            if member.is_some() && game::set_specialties(i, bits) {
                T::SpecialtiesLater.get(lang).to_owned()
            } else {
                T::NoMember.get(lang).to_owned()
            }
        }
        What::CrewLicence => {
            let level = state.menu.value(What::CrewLicence).unwrap_or(1.0) as u32;
            match member {
                None => T::NoMember.get(lang).to_owned(),
                Some(m) if game::set_licence(m, level) => {
                    format!("{}{level}", T::LicenceSet.get(lang))
                }
                Some(_) => T::NoLicence.get(lang).to_owned(),
            }
        }
        What::CrewEscort => match game::escort() {
            Some(c) => {
                let n = state.menu.value(What::CrewEscort).unwrap_or(3.0) as u32;
                game::set_escort_count(c, n);
                format!("{}: {n}", T::CrewEscort.get(lang))
            }
            None => T::NoEscort.get(lang).to_owned(),
        },
        _ => return,
    };
    set_status(state, text);
}

/// The weapon items, on the gun in the player's hands only: a gun the
/// player has dropped may be gone, so the mod writes nothing to it; what it
/// changed on a gun put away is put back when that gun is in the hands
/// again with the item off.
fn weapons(state: &mut State) {
    let menu = &state.menu;
    let keep = &mut state.keep;
    let gun = game::gun();
    hooks::PLAYER_HANDLE.store(game::player_handle().unwrap_or(0), Ordering::Relaxed);
    let k = menu.value(What::NpcDamage).unwrap_or(1.0);
    hooks::ENEMY_DAMAGE.store(k.to_bits(), Ordering::Relaxed);
    let no_reload = menu.on(What::NoReload);
    hooks::NO_RELOAD_GUN.store(
        if no_reload { gun.unwrap_or(0) } else { 0 },
        Ordering::Relaxed,
    );
    let Some(g) = gun else {
        return;
    };
    // A gun no item holds any more is forgotten (the maps stay small; the
    // mod never writes to a gun outside the hands anyway).
    const KEEP_AT_MOST: usize = 32;
    for map_len in [keep.ammo.len(), keep.scale.len(), keep.camera.len()] {
        if map_len > KEEP_AT_MOST {
            keep.ammo.retain(|&k, _| k == g);
            keep.scale.retain(|&k, _| k == g);
            keep.camera.retain(|&k, _| k == g);
            break;
        }
    }
    // Infinite ammunition: the game's own flag.
    let flags = game::gun_flags(g);
    if menu.on(What::InfiniteAmmo) {
        if flags & game::GUN_INFINITE != game::GUN_INFINITE {
            keep.ammo.entry(g).or_insert(flags & game::GUN_INFINITE);
            game::set_gun_flags(g, flags | game::GUN_INFINITE);
        }
    } else if let Some(bits) = keep.ammo.remove(&g) {
        game::set_gun_flags(g, flags & !game::GUN_INFINITE | bits);
    }
    // Damage, rate of fire (the interval is divided), spread.
    let damage = menu.value(What::Damage).unwrap_or(1.0);
    let rate = menu.value(What::FireRate).unwrap_or(1.0).max(0.1);
    let spread = menu.value(What::Spread).unwrap_or(1.0);
    let factors = [damage, 1.0 / rate, spread, spread];
    let base = game::GUN_BASE.map(|o| game::gun_f32(g, o));
    let own = match keep.scale.get(&g) {
        // The game has not touched what the mod wrote.
        Some(&(own, written)) if base == written => own,
        // New, or the game wrote its own (a level up): that is the gun's.
        _ => base,
    };
    if factors == [1.0; 4] {
        if keep.scale.remove(&g).is_some() && base != own {
            for (i, &o) in own.iter().enumerate() {
                game::set_gun_f32(g, game::GUN_BASE[i], o);
                game::set_gun_f32(g, game::GUN_EFFECTIVE[i], o);
            }
        }
    } else {
        let want = [0, 1, 2, 3].map(|i| own[i] * factors[i]);
        if base != want {
            for (i, &v) in want.iter().enumerate() {
                game::set_gun_f32(g, game::GUN_BASE[i], v);
                game::set_gun_f32(g, game::GUN_EFFECTIVE[i], v);
            }
        }
        keep.scale.insert(g, (own, want));
    }
    // No camera shake or kick at a shot.
    let camera = game::GUN_CAMERA.map(|o| game::gun_f32(g, o));
    if menu.on(What::NoRecoil) {
        if camera != [0.0; 2] {
            keep.camera.entry(g).or_insert(camera);
            for o in game::GUN_CAMERA {
                game::set_gun_f32(g, o, 0.0);
            }
        }
    } else if let Some(was) = keep.camera.remove(&g) {
        for (i, o) in game::GUN_CAMERA.into_iter().enumerate() {
            game::set_gun_f32(g, o, was[i]);
        }
    }
}

/// Holds the toggles in the game every tick: the game may undo them (a
/// cut-scene ends invulnerability, a new life has a new damage component,
/// a level a new HUD).
fn keep(state: &mut State, mode: u32) {
    let damage = game::damage();
    // God mode.
    let god = state.menu.on(What::GodMode);
    if let Some((d, added)) = state.keep.god
        && (!god || damage != Some(d))
    {
        if damage == Some(d) {
            for _ in 0..added {
                game::set_invulnerable(d, false);
            }
        }
        state.keep.god = None;
    }
    if god && let Some(d) = damage {
        let added = state.keep.god.map_or(0, |(_, n)| n);
        let mut now = added;
        if !game::invulnerable(d) {
            game::set_invulnerable(d, true);
            now += 1;
        }
        state.keep.god = Some((d, now));
        // Some blows pass invulnerability (bit 6 of their flags): health
        // back to its maximum.
        let (health, max) = game::health(d);
        if health > 0.0 && health < max {
            game::set_health(d, max);
        }
    }
    // One hit and down.
    let one_hit = state.menu.on(What::OneHit);
    if let Some((d, max)) = state.keep.one_hit
        && (!one_hit || damage != Some(d))
    {
        if damage == Some(d) {
            // The maximum comes back; health grows back by itself.
            game::set_max_health(d, max);
        }
        state.keep.one_hit = None;
    }
    if one_hit && let Some(d) = damage {
        let (_, max) = game::health(d);
        if state.keep.one_hit.is_none() {
            state.keep.one_hit = Some((d, max));
        }
        if max != 1.0 {
            game::set_max_health(d, 1.0);
        }
    }
    // The HUD.
    let hide = !state.menu.on(What::Hud);
    let hud = game::hud();
    if let Some(h) = state.keep.hud
        && (!hide || hud != Some(h))
    {
        if hud == Some(h) {
            game::hud_hide(h, false);
        }
        state.keep.hud = None;
    }
    if hide
        && state.keep.hud.is_none()
        && game::player().is_some()
        && let Some(h) = hud
    {
        game::hud_hide(h, true);
        state.keep.hud = Some(h);
    }
    weapons(state);
    // A teleport waits for the world to move.
    if let Some(t) = state.keep.teleport
        && mode == RUNNING
    {
        teleport(state, t);
    }
}

/// The ground under the waypoint from the game's physics: the player goes
/// there at once when the physics has it; otherwise the player hangs above
/// the waypoint (the game does not drop a teleported player) while the game
/// loads the zones around, and goes down when the ray finds the ground.
fn teleport(state: &mut State, mut t: Teleport) {
    let done = |state: &mut State, text: T| {
        state.keep.teleport = None;
        let text = text.get(state.lang).to_owned();
        set_status(state, text);
    };
    let started = *t.started.get_or_insert_with(Instant::now);
    if started.elapsed().as_secs_f32() > TELEPORT_SECONDS {
        note("teleport: the game did not move the player in time");
        return done(state, T::NoTeleport);
    }
    // Back where the player was when the mod has moved him, else nowhere.
    let from = t.from;
    let back = |hung: bool, text: T| match (hung, from) {
        (true, Some(at)) => Step::Move(at, text),
        _ => Step::Move([f32::NAN; 3], text),
    };
    t.step = match t.step {
        Step::Find | Step::Hang(_) => {
            let hung = matches!(t.step, Step::Hang(_));
            match game::ground(t.x, t.z, RAY_TOP, RAY_BOTTOM) {
                Some(y) if y < SEA_FLOOR => {
                    note(&format!(
                        "teleport: water under {:.1}, {:.1} (floor {y:.2})",
                        t.x, t.z
                    ));
                    back(hung, T::Water)
                }
                Some(y) => {
                    note(&format!(
                        "teleport: ground at {y:.2} under {:.1}, {:.1}",
                        t.x, t.z
                    ));
                    Step::Move([t.x, y + ABOVE_GROUND, t.z], T::Teleported)
                }
                None => match t.step {
                    Step::Hang(since) if since.elapsed().as_secs_f32() > HANG_SECONDS => {
                        note("teleport: no ground under the waypoint");
                        back(true, T::NoGround)
                    }
                    Step::Hang(since) => Step::Hang(since),
                    _ => {
                        let y = game::position().map_or(HANG, |p| p[1].max(HANG));
                        if game::teleport([t.x, y, t.z]).is_ok() {
                            note(&format!(
                                "teleport: no ground yet under {:.1}, {:.1}; waiting at {y:.1}",
                                t.x, t.z
                            ));
                            let text = T::TeleportWait.get(state.lang).to_owned();
                            set_status(state, text);
                            Step::Hang(Instant::now())
                        } else {
                            Step::Find
                        }
                    }
                },
            }
        }
        Step::Move(to, text) => {
            if to[0].is_nan() {
                return done(state, text);
            }
            match game::teleport(to) {
                Ok(()) => {
                    note(&format!(
                        "teleport: moved to {:.1}, {:.2}, {:.1}",
                        to[0], to[1], to[2]
                    ));
                    return done(state, text);
                }
                // The game's own teleport still runs: next tick.
                Err(_) => Step::Move(to, text),
            }
        }
    };
    state.keep.teleport = Some(t);
}

unsafe extern "C" fn on_tick(_: *mut c_void, mode: u32) {
    let Some(p) = Platform::get() else {
        return;
    };
    let Some(input) = p.input() else {
        return;
    };
    let Ok(mut guard) = STATE.lock() else {
        return;
    };
    let Some(state) = guard.as_mut() else {
        return;
    };
    keep(state, mode);
    let down = opener(&input, &state.config);
    let pressed = down && !state.opener_down;
    state.opener_down = down;
    if pressed {
        let open = !state.open;
        open_menu(&p, state, open);
        if open {
            // The keys held when the menu opens do nothing in it.
            let now = ms(state);
            state.repeat.feed(&held(&input, state.config.pad), now);
            if let Some(hour) = game::hour() {
                state.menu.set_value(What::Hour, hour as f32);
            }
        }
        return;
    }
    if !state.open {
        return;
    }
    state.coords = game::position();
    crew(state);
    let now = ms(state);
    let keys = held(&input, state.config.pad);
    for key in state.repeat.feed(&keys, now) {
        match state.menu.press(key) {
            Effect::Close => {
                open_menu(&p, state, false);
                return;
            }
            Effect::Run(what) => run(state, what),
            Effect::NotYet => {
                let text = T::NotYet.get(state.lang).to_owned();
                set_status(state, text);
            }
            Effect::Changed(what) => changed(state, what),
            Effect::Moved | Effect::None => {}
        }
    }
    if let Some((_, until)) = state.status
        && now.wrapping_sub(until) < u32::MAX / 2
    {
        state.status = None;
    }
}

unsafe extern "C" fn on_overlay(_: *mut c_void, overlay: *const abi::Overlay) {
    let Ok(guard) = STATE.lock() else {
        return;
    };
    let Some(state) = guard.as_ref() else {
        return;
    };
    let Ok(mut painter) = PAINTER.lock() else {
        return;
    };
    // SAFETY: the platform gives a valid description for the call.
    let Some(mut frame) = painter.frame(unsafe { &*overlay }) else {
        return;
    };
    let status = state.status.as_ref().map(|(s, _)| s.as_str());
    let coords = state.coords;
    let info = |what: What| match what {
        What::Coords => coords.map(view::coords),
        What::CrewPlace => state.place_label.clone(),
        _ => None,
    };
    view::draw(&mut frame, &state.menu, state.lang, status, &info);
}

/// # Safety
/// Called by `doctor_platform.asi` with its table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn doctor_mod_init(api: *const Api) -> i32 {
    // SAFETY: the table of the platform.
    let Some(p) = (unsafe { Platform::new(api) }) else {
        return abi::ERR_STATE;
    };
    let dir = module_dir();
    let config = dir
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join(config::FILE)).ok())
        .map(|t| Config::parse(&t))
        .unwrap_or_default();
    let settings = std::env::var_os("LOCALAPPDATA")
        .map(|d| text::settings_files(std::path::Path::new(&d)))
        .unwrap_or_default();
    let lang = text::language(config.language, &settings, registry_locale());
    let fonts = PAINTER
        .lock()
        .ok()
        .and_then(|mut painter| painter.atlas().map(|a| (a.width, a.height)));
    let reserved = p.reserve_key(config.key, true);
    let (no_reload, enemy_damage) = hooks::install(&p);
    let tick = p.tick_listen(on_tick, std::ptr::null_mut());
    let overlay = p.overlay_listen(on_overlay, std::ptr::null_mut(), 0);
    if overlay > 0 {
        OVERLAY.store(overlay, Ordering::Relaxed);
        p.overlay_show(overlay, false);
    }
    if let Ok(mut s) = STATE.lock() {
        *s = Some(State {
            config: config.clone(),
            lang,
            menu: {
                let mut menu = Menu::default();
                menu.set_ready(What::NoReload, no_reload);
                menu.set_ready(What::NpcDamage, enemy_damage);
                menu.set_ready(What::Teleport, game::ray_ready());
                if config.remember {
                    menu.restore(&config.values);
                    menu.set_toggle(What::Remember, true);
                }
                menu
            },
            open: false,
            repeat: Repeat::default(),
            opener_down: false,
            status: None,
            started: Instant::now(),
            keep: Keep::default(),
            coords: None,
            place_label: None,
            place_name: None,
            config_path: dir.as_ref().map(|d| d.join(config::FILE)),
        });
    }
    log(
        &p,
        &format!(
            "{VERSION} on {}: key {:#04x} (reserved {reserved}), pad {}, language {lang:?}, \
             fonts {fonts:?}, tick {tick}, overlay {overlay}, hooks: no reload {no_reload}, \
             enemy damage {enemy_damage}, remembered {}",
            p.edition().to_string_lossy(),
            config.key,
            config.pad,
            config.remember,
        ),
    );
    if tick <= 0 || overlay <= 0 || fonts.is_none() {
        return abi::ERR_STATE;
    }
    abi::OK
}
