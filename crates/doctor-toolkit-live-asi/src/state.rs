//! The world and the state of the game (stage 48): time of day, weather,
//! streams loaded one at a time, money, the family, missions and the
//! variables of the story (`docs/analysis/live-link.md` §13).
//!
//! Reads run on the main thread between frames like the other jobs; writes
//! run inside a level frame (the update hook), where the game itself sets
//! the time, books money and asks for streams. Every call is one the game
//! makes in the same place with the same arguments.

use std::sync::Mutex;

use doctor_toolkit_live::addr::*;
use doctor_toolkit_live::state::{STREAM_HISTORY, STREAM_LOADED, STREAM_PATIENCE_S, stream_name};
use doctor_toolkit_live::{
    Checkpoint, CrewSlot, Done, ErrorCode, Family, FamilyInfo, Mission, Money, MoneyChange, Rain,
    Response, StreamAction, StreamQueue, StreamRequest, TimeChange, TimeOfDay, Variable, Weather,
};

use crate::game;
use crate::win::Mem;

fn not_ready(why: &str) -> Response {
    Response::error(ErrorCode::NotReady, why)
}

fn f32_at(m: &mut Mem, a: u32) -> Option<f32> {
    m.u32(a).map(f32::from_bits)
}

// --- Time of day --------------------------------------------------------------

fn clock(m: &mut Mem) -> Option<(u32, TimeOfDay)> {
    let tod = m.ptr(TIME_OF_DAY)?;
    let year = m.u32(tod + TIME_OF_DAY_YEAR)?;
    let day = m.u32(tod + TIME_OF_DAY_YEAR + 4)?;
    let h = m.u32(tod + TIME_OF_DAY_HMS)?;
    let min = m.u32(tod + TIME_OF_DAY_HMS + 4)?;
    let s = m.u32(tod + TIME_OF_DAY_HMS + 8)?;
    if h > 23 || min > 59 || s > 59 {
        return None;
    }
    Some((
        tod,
        TimeOfDay {
            year,
            day,
            hour: h as u8,
            minute: min as u8,
            second: s as u8,
            rate: f32_at(m, tod + TIME_OF_DAY_RATE)?,
            default_rate: f32_at(m, tod + TIME_OF_DAY_DEFAULT_RATE)?,
        },
    ))
}

pub fn time_of_day() -> Response {
    let mut m = Mem::default();
    Response::TimeOfDay(clock(&mut m).map(|c| c.1))
}

/// Sets the clock as `SetTimeOfDay` does (`0x00825D43`): the setter with the
/// year and day it has, then the rate.
pub fn set_time(change: TimeChange) -> Response {
    let mut m = Mem::default();
    let Some((tod, now)) = clock(&mut m) else {
        return not_ready("no TimeOfDayManager in this level");
    };
    if let Some(rate) = change.rate
        && (!rate.is_finite() || rate > 100_000.0)
    {
        return Response::error(ErrorCode::BadRequest, "rate out of range");
    }
    if let Some([h, min, s]) = change.time {
        // SAFETY: `SetTime` of the TimeOfDayManager (checked bytes), on the
        // main thread inside a level frame, as `SetTimeOfDay` calls it.
        unsafe {
            let set: extern "thiscall" fn(u32, u32, u32, u32, u32, u32) =
                std::mem::transmute(TIME_OF_DAY_SET as usize);
            set(
                tod,
                now.year,
                now.day,
                u32::from(h),
                u32::from(min),
                u32::from(s),
            );
        }
    }
    if let Some(rate) = change.rate {
        let value = if rate < 0.0 { now.default_rate } else { rate };
        // SAFETY: the rate of the manager read above, a float the game
        // writes the same way (`0x00825D6A`).
        unsafe {
            std::ptr::write_volatile((tod + TIME_OF_DAY_RATE) as usize as *mut f32, value);
        }
    }
    let after = clock(&mut m).map(|c| c.1);
    Response::Done(Done {
        count: 1,
        note: match after {
            Some(t) => format!(
                "time of day {:02}:{:02}:{:02}, rate {} (was {:02}:{:02}:{:02}, rate {})",
                t.hour, t.minute, t.second, t.rate, now.hour, now.minute, now.second, now.rate
            ),
            None => "time set".into(),
        },
    })
}

pub fn weather() -> Response {
    let mut m = Mem::default();
    let rain = m.ptr(WEATHER_MANAGER).and_then(|w| {
        Some(Rain {
            flags: m.u32(w + WEATHER_FLAGS)? & 0xFFFF,
            length: f32_at(&mut m, w + WEATHER_RAIN_LENGTH)?,
            elapsed: f32_at(&mut m, w + WEATHER_RAIN_LENGTH + 4)?,
        })
    });
    Response::Weather(Weather {
        rain,
        time: clock(&mut m).map(|c| c.1),
    })
}

// --- Streams -------------------------------------------------------------------

struct Requests {
    list: Vec<StreamRequest>,
    /// Frame of each request.
    since: Vec<(u32, std::time::Instant)>,
    /// Streams the link loaded (it unloads only these unless forced).
    loaded: Vec<String>,
}

static STREAMS: Mutex<Requests> = Mutex::new(Requests {
    list: Vec::new(),
    since: Vec::new(),
    loaded: Vec::new(),
});

fn c_name(name: &str) -> Vec<u8> {
    let mut v: Vec<u8> = name.bytes().filter(|&b| b != 0).collect();
    v.push(0);
    v
}

/// Id and state of a stream by name: `0x00405900` then `0x004025A0`
/// (what `UnloadStream` and `0x00813C10` do). The game knows the names
/// without `.str` (`gf2\cuba\local`); the list of loaded streams shows them
/// with it.
fn lookup(manager: u32, name: &str) -> (u32, u32) {
    let mut m = Mem::default();
    for candidate in [name.to_owned(), format!("{name}.str")] {
        let c = c_name(&candidate);
        // SAFETY: the lookups of the StreamManager (checked bytes) with a
        // NUL-terminated name, on the main thread.
        let id = unsafe {
            let id_of: extern "thiscall" fn(u32, *const u8, u32) -> u32 =
                std::mem::transmute(STREAM_ID_OF as usize);
            id_of(manager, c.as_ptr(), 0)
        };
        if id == 0 {
            continue;
        }
        // SAFETY: as above.
        let record = unsafe {
            let record_of: extern "thiscall" fn(u32, u32) -> u32 =
                std::mem::transmute(STREAM_RECORD_OF as usize);
            record_of(manager, id)
        };
        let state = if record >= 0x1_0000 {
            m.u32(record + STREAM_RECORD_STATE).unwrap_or(0)
        } else {
            0
        };
        return (id, state);
    }
    (0, 0)
}

fn refresh(manager: u32, r: &mut Requests) {
    let frame = crate::frames();
    for (req, (since, at)) in r.list.iter_mut().zip(&r.since) {
        if req.done || req.expired {
            continue;
        }
        let (id, state) = lookup(manager, &req.name);
        req.id = id;
        req.state = state;
        req.frames = frame.wrapping_sub(*since);
        req.done = match req.action {
            StreamAction::Load => state == STREAM_LOADED,
            StreamAction::Unload => id == 0 || state != STREAM_LOADED,
        };
        req.expired = !req.done && at.elapsed().as_secs() >= STREAM_PATIENCE_S;
    }
}

/// Whether the stream's file is in the game folder (`gf2\cuba\local` →
/// `gf2/cuba/local.str` next to `scripts/`): for a name it does not know
/// the streamer keeps a record "asked for" (state 1) for ever, and the
/// link's next request would wait for it.
fn stream_file_exists(name: &str) -> bool {
    if name.contains(':') {
        // `embedded:\…` streams live inside the executable's data.
        return true;
    }
    let Some(root) = crate::module_dir()
        .parent()
        .map(std::path::Path::to_path_buf)
    else {
        return true;
    };
    let mut path = root;
    for part in name.split('\\').filter(|p| !p.is_empty()) {
        path.push(part);
    }
    path.set_extension("str");
    path.is_file()
}

fn busy(m: &mut Mem, manager: u32) -> bool {
    m.u32(manager + STREAM_LOADING).is_some_and(|p| p != 0)
}

pub fn stream_queue() -> Response {
    let mut m = Mem::default();
    let Some(manager) = m.ptr(STREAM_MANAGER) else {
        return not_ready("no StreamManager");
    };
    let Ok(mut r) = STREAMS.lock() else {
        return Response::error(ErrorCode::Failed, "stream requests are poisoned");
    };
    refresh(manager, &mut r);
    Response::StreamQueue(StreamQueue {
        busy: busy(&mut m, manager),
        requests: r.list.clone(),
    })
}

/// Loads or unloads one stream, after the earlier request of the link is
/// done and while the game loads nothing: two requests at once crash the
/// game (`launch-flags.md`, `0x0042138B`, `0x00890CEB`).
pub fn stream(name: &str, action: StreamAction, force: bool) -> Response {
    let name = stream_name(name);
    if name.is_empty() || name.len() > 250 {
        return Response::error(ErrorCode::BadRequest, "no stream name");
    }
    let mut m = Mem::default();
    let Some(manager) = m.ptr(STREAM_MANAGER) else {
        return not_ready("no StreamManager");
    };
    let Ok(mut r) = STREAMS.lock() else {
        return Response::error(ErrorCode::Failed, "stream requests are poisoned");
    };
    refresh(manager, &mut r);
    if let Some(open) = r.list.iter().find(|q| !q.done && !q.expired) {
        return not_ready(&format!(
            "the stream {} of the link is not {} yet (state {}, {} frames); one request at a time",
            open.name,
            match open.action {
                StreamAction::Load => "loaded",
                StreamAction::Unload => "unloaded",
            },
            open.state,
            open.frames
        ));
    }
    if busy(&mut m, manager) {
        return not_ready("the game is loading a stream now");
    }
    let (id, state) = lookup(manager, &name);
    let note = match action {
        StreamAction::Load => {
            if id != 0 && state == STREAM_LOADED {
                return Response::Done(Done {
                    count: 0,
                    note: format!("{name} is loaded already (id {id})"),
                });
            }
            if !stream_file_exists(&name) {
                return Response::error(
                    ErrorCode::Failed,
                    format!("the game folder has no stream {name}.str"),
                );
            }
            let c = c_name(&name);
            // SAFETY: the request `LoadStream` makes (`0x00813A1F`, checked
            // bytes): name, priority, zeros; inside a level frame.
            let got = unsafe {
                let request: extern "thiscall" fn(u32, *const u8, f32, u32, u32, u32) -> u32 =
                    std::mem::transmute(STREAM_REQUEST as usize);
                request(manager, c.as_ptr(), STREAM_PRIORITY, 0, 0, 0)
            };
            if got == 0 {
                return Response::error(
                    ErrorCode::Failed,
                    format!("the game knows no stream {name} (no entry in the tables of contents)"),
                );
            }
            if !r.loaded.contains(&name) {
                r.loaded.push(name.clone());
            }
            format!("asked for {name} (id {got:#x}); `live stream list` tells when it is loaded")
        }
        StreamAction::Unload => {
            if id == 0 || state != STREAM_LOADED {
                return Response::error(
                    ErrorCode::Failed,
                    format!("{name} is not loaded (state {state})"),
                );
            }
            if !force && !r.loaded.contains(&name) {
                return Response::error(
                    ErrorCode::Denied,
                    format!(
                        "{name} was not loaded by the link; unloading a stream the game uses leaves its objects dangling (force to do it anyway)"
                    ),
                );
            }
            // SAFETY: the record of a loaded stream, unloaded the way
            // `UnloadStream` does (`0x00813BFB`), inside a level frame.
            unsafe {
                let record_of: extern "thiscall" fn(u32, u32) -> u32 =
                    std::mem::transmute(STREAM_RECORD_OF as usize);
                let record = record_of(manager, id);
                if record < 0x1_0000 {
                    return Response::error(ErrorCode::Failed, "no record of the stream");
                }
                let unload: extern "thiscall" fn(u32, u32) =
                    std::mem::transmute(STREAM_UNLOAD as usize);
                unload(manager, record);
            }
            r.loaded.retain(|n| n != &name);
            format!("unloading {name} (id {id:#x})")
        }
    };
    r.list.push(StreamRequest {
        name,
        action,
        id,
        state,
        done: false,
        frames: 0,
        expired: false,
    });
    r.since.push((crate::frames(), std::time::Instant::now()));
    while r.list.len() > STREAM_HISTORY {
        r.list.remove(0);
        r.since.remove(0);
    }
    Response::Done(Done { count: 1, note })
}

// --- Money and family ------------------------------------------------------------

fn families(m: &mut Mem) -> Vec<u32> {
    let Some(f) = m.ptr(FAMILIES) else {
        return Vec::new();
    };
    let (Some(list), Some(n)) = (m.ptr(f + FAMILIES_LIST), m.u32(f + FAMILIES_COUNT)) else {
        return Vec::new();
    };
    (0..n.min(256))
        .filter_map(|i| m.ptr(list + i * 4))
        .collect()
}

fn player_family(m: &mut Mem) -> Option<u32> {
    families(m)
        .into_iter()
        .find(|&f| m.u32(f + FAMILY_NAME) == Some(PLAYER_FAMILY))
}

fn money_of(m: &mut Mem, family: u32) -> Option<Money> {
    let cash = f32_at(m, family + FAMILY_CASH)?;
    let mut books = Vec::new();
    if let Some(b) = m.ptr(family + FAMILY_BOOKS) {
        let n = m.u32(b + BOOKS_CATEGORY_COUNT).unwrap_or(0).min(11);
        for i in 0..n {
            books.push(f32_at(m, b + BOOKS_CATEGORIES + i * 4).unwrap_or(0.0));
        }
    }
    Some(Money { cash, books })
}

pub fn money() -> Response {
    let mut m = Mem::default();
    Response::Money(player_family(&mut m).and_then(|f| money_of(&mut m, f)))
}

/// Books money to the player's family with the call of the game's rewards
/// (`0x008C09A0`): the counter on screen follows.
pub fn set_money(change: MoneyChange) -> Response {
    let mut m = Mem::default();
    let Some(family) = player_family(&mut m) else {
        return not_ready("no family of the player");
    };
    let Some(before) = f32_at(&mut m, family + FAMILY_CASH) else {
        return not_ready("the family's money cannot be read");
    };
    let amount = match change {
        MoneyChange::Add(v) => v,
        MoneyChange::Set(v) => v - before,
    };
    if !amount.is_finite() || amount.abs() > 1.0e9 {
        return Response::error(ErrorCode::BadRequest, "amount out of range");
    }
    // SAFETY: `AddMoney` of the player's family (checked bytes), with the
    // arguments the game's rewards pass, inside a level frame.
    let after = unsafe {
        let add: extern "thiscall" fn(u32, f32, u32) -> f32 =
            std::mem::transmute(FAMILY_ADD_MONEY as usize);
        add(family, amount, MONEY_CATEGORY)
    };
    Response::Done(Done {
        count: 1,
        note: format!("money {after} (was {before}, {amount:+})"),
    })
}

pub fn family() -> Response {
    let mut m = Mem::default();
    let list = families(&mut m)
        .into_iter()
        .filter_map(|f| {
            Some(FamilyInfo {
                hash: m.u32(f + FAMILY_NAME)?,
                cash: f32_at(&mut m, f + FAMILY_CASH)?,
            })
        })
        .collect();
    let mut crew = Vec::new();
    if let Some(c) = m.ptr(CREW) {
        for i in 0..CREW_SLOT_COUNT {
            let at = c + CREW_SLOTS + i * CREW_SLOT_SIZE;
            let (Some(flags), Some(rank)) = (m.u32(at), m.u32(at + 4)) else {
                break;
            };
            let member = (flags & 2 != 0)
                .then(|| m.ptr(at + 8))
                .flatten()
                .map(|h| h - 0x48);
            crew.push(CrewSlot {
                slot: i as u8,
                flags,
                rank,
                member: member.and_then(|p| game::object_guid(&mut m, p)),
                member_rank: member.and_then(|p| m.u32(p + MEMBER_RANK)).unwrap_or(0),
                specialties: if member.is_some() {
                    m.u32(at + 0x10).unwrap_or(0) & 0xFC
                } else {
                    0
                },
            });
        }
    }
    Response::Family(Family {
        families: list,
        crew,
    })
}

// --- Missions ------------------------------------------------------------------------

fn checkpoint(m: &mut Mem, p: u32) -> Option<Checkpoint> {
    if m.u32(p)? != CHECKPOINT_VTABLE {
        return None;
    }
    Some(Checkpoint {
        number: m.u32(p + CHECKPOINT_NUMBER)?,
        chapter: m.u32(p + CHECKPOINT_CHAPTER)?,
        guid: game::object_guid(m, p)?,
    })
}

pub fn mission() -> Response {
    let mut m = Mem::default();
    let Some(cm) = m.ptr(CHECKPOINTS) else {
        return Response::Mission(Mission::default());
    };
    let current = m
        .ptr(cm + CHECKPOINT_CURRENT)
        .and_then(|p| checkpoint(&mut m, p));
    let mut checkpoints = Vec::new();
    if let (Some(list), Some(n)) = (m.ptr(cm + CHECKPOINT_LIST), m.u32(cm + CHECKPOINT_COUNT)) {
        for i in 0..n.min(1024) {
            if let Some(c) = m.ptr(list + i * 4).and_then(|p| checkpoint(&mut m, p)) {
                checkpoints.push(c);
            }
        }
    }
    Response::Mission(Mission {
        current,
        checkpoints,
    })
}

// --- Variables --------------------------------------------------------------------------

fn all_variables(m: &mut Mem) -> Option<(u32, Vec<Variable>)> {
    let vm = m.ptr(VARIABLES)?;
    let entries = m.ptr(vm + VARIABLES_ENTRIES)?;
    let room = m.u32(vm + VARIABLES_ROOM)?;
    let n = m.u32(vm + VARIABLES_COUNT)?.min(room).min(4096);
    let mut buf = vec![0u8; (n * VARIABLE_SIZE) as usize];
    if !m.bytes(entries, &mut buf) {
        return None;
    }
    let at = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    let list = (0..n as usize)
        .map(|i| {
            let o = i * VARIABLE_SIZE as usize;
            Variable {
                hash: at(o),
                name_hash: at(o + 4),
                value: at(o + 8) as i32,
                kind: u16::from_le_bytes([buf[o + 0xE], buf[o + 0xF]]),
            }
        })
        .collect();
    Some((vm, list))
}

pub fn variables(hashes: &[u32]) -> Response {
    let mut m = Mem::default();
    let Some((_, list)) = all_variables(&mut m) else {
        return not_ready("no variables (not in a level)");
    };
    Response::Variables(
        list.into_iter()
            .filter(|v| hashes.is_empty() || hashes.iter().any(|&h| v.is(h)))
            .collect(),
    )
}

/// Sets a variable as Lua `VariableSet` does (`0x0046A666`): the value, then
/// `0x00442240(manager, index)` wakes the `VariableWatcher`s.
pub fn set_variable(hash: u32, value: i32) -> Response {
    let mut m = Mem::default();
    let Some((vm, list)) = all_variables(&mut m) else {
        return not_ready("no variables (not in a level)");
    };
    let Some(index) = list.iter().position(|v| v.is(hash)) else {
        return Response::error(
            ErrorCode::Failed,
            format!("no variable {hash:#010x} in the loaded streams"),
        );
    };
    let before = list[index].value;
    let Some(entries) = m.ptr(vm + VARIABLES_ENTRIES) else {
        return not_ready("no variables");
    };
    // SAFETY: the value of an entry inside the count (read above), and the
    // call Lua `VariableSet` makes after it (checked bytes), in a frame.
    unsafe {
        std::ptr::write_volatile(
            (entries + index as u32 * VARIABLE_SIZE + 8) as usize as *mut i32,
            value,
        );
        let changed: extern "stdcall" fn(u32, u32) = std::mem::transmute(VARIABLE_CHANGED as usize);
        changed(vm, index as u32);
    }
    Response::Done(Done {
        count: 1,
        note: format!("variable {hash:#010x} = {value} (was {before})"),
    })
}
