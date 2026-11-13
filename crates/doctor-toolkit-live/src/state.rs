//! The world and the state of the game (stage 48, protocol 4): time of day,
//! weather, streams loaded one at a time, money, the family, missions and
//! the variables of the story — read, and written for checks without
//! playing through (`docs/analysis/live-link.md` §13).

use crate::Guid;
use crate::proto::DecodeError;
use crate::wire::{Reader, Writer};

/// The game's clock (`TimeOfDayManager`, `*0x0112A53C`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeOfDay {
    /// Year (`+0x5C`, 1950 in the story) and day of the year (`+0x60`,
    /// 0–364).
    pub year: u32,
    pub day: u32,
    /// `+0x64`, `+0x68`, `+0x6C`.
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Game seconds per real second (`+0x70`; `SetTimeOfDay` writes it).
    pub rate: f32,
    /// The rate of the level's `TimeOfDayManager` (`+0x74`, 60 in
    /// `gf2/globals_global`); `SetTimeOfDay` with flag 2 goes back to it.
    pub default_rate: f32,
}

/// What [`crate::Request::SetTime`] changes; `None` keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TimeChange {
    /// Hours, minutes, seconds: through the setter `SetTimeOfDay` uses
    /// (`0x008269F0`), so sky, fog and light follow at once.
    pub time: Option<[u8; 3]>,
    /// Game seconds per real second; a negative value goes back to the
    /// level's own rate. 0 stops the clock.
    pub rate: Option<f32>,
}

/// The rain of `WeatherManager` (`*0x0112AFF4`, `0x00836CC0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rain {
    /// Flags `+0x84`: bit 0 it rains, bit 1 random rain is off, bit 2 the
    /// rain was announced (`iMsgStartRainFadeIn`), bit 4 under a roof.
    pub flags: u32,
    /// Seconds the current rain lasts (`+0xB0`) and has lasted (`+0xB4`).
    pub length: f32,
    pub elapsed: f32,
}

/// Answer to [`crate::Request::Weather`]: what decides the sky.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Weather {
    /// The rain manager, when the level has one. No SimGroup of the game
    /// has a `WeatherManager` object (all 2 006 resources): in the single
    /// player game it is `None`.
    pub rain: Option<Rain>,
    /// Sky, fog and light are keys of the time of day (`TODSequencer`:
    /// `SetSkyMessage`, `SetFogMessage`, `LightingTODMessage` …); the
    /// clock decides them.
    pub time: Option<TimeOfDay>,
}

/// Whether a stream request loads or unloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamAction {
    Load = 0,
    Unload = 1,
}

/// One request of the link to the streamer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRequest {
    pub name: String,
    pub action: StreamAction,
    /// Id of the stream (`0x00405900`), 0 when it has none (not known or
    /// unloaded).
    pub id: u32,
    /// State of its record (`+0x24`, `0x00813C4A`): 1 asked for, 3 loaded;
    /// 2 and 4 on the way out (`0x00403C40`); 0 when there is no record.
    pub state: u32,
    /// Whether the request is done: loaded, or gone.
    pub done: bool,
    /// Frames of the main loop since the request.
    pub frames: u32,
    /// The request was not done in [`STREAM_PATIENCE_S`] seconds: it no
    /// longer holds the next one back.
    pub expired: bool,
}

/// State of a stream record that is loaded.
pub const STREAM_LOADED: u32 = 3;
/// Seconds a request of the link may hold the next one back.
pub const STREAM_PATIENCE_S: u64 = 120;

/// Answer to [`crate::Request::StreamQueue`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamQueue {
    /// The game is creating the objects of a stream now
    /// (`StreamManager+0x84`): no request is made then.
    pub busy: bool,
    /// Requests of the link, the newest last (at most
    /// [`STREAM_HISTORY`]).
    pub requests: Vec<StreamRequest>,
}

/// Requests a module remembers.
pub const STREAM_HISTORY: usize = 16;

/// The game's spelling of a stream name: `\` between folders, no `.str`
/// (`gf2/cuba/local.str` → `gf2\cuba\local`).
pub fn stream_name(text: &str) -> String {
    let t = text.trim().replace('/', "\\");
    let lower = t.to_ascii_lowercase();
    match lower.strip_suffix(".str") {
        Some(_) => t[..t.len() - 4].to_owned(),
        None => t,
    }
}

/// A change of money.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoneyChange {
    Add(f32),
    Set(f32),
}

/// The player's money: his family's (`Corleone`, `0x0637B907`).
#[derive(Debug, Clone, PartialEq)]
pub struct Money {
    /// The money the interface shows (`Family+0x100`, copied to
    /// `*0x01129908 + 0x14` for the counter, `0x008BCAF0`).
    pub cash: f32,
    /// The family's books (`Family+0x230`): income by category as
    /// `0x00892910` counts it, when the family keeps them.
    pub books: Vec<f32>,
}

/// The family of the player and the others.
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyInfo {
    /// Hash of its name (`Family+0x54`): `corleone` `0x0637B907`.
    pub hash: u32,
    /// `Family+0x100`.
    pub cash: f32,
}

/// A place of the player's crew (`*0x0112989C + 0x18`, 7 × 0x24 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewSlot {
    pub slot: u8,
    /// Bit 0: the place is open; bit 1: a member holds it.
    pub flags: u32,
    /// Rank the place asks for (`+4`): 3 soldier, 4 capo, 5 underboss.
    pub rank: u32,
    /// The member: GUID of his SimGroup object (`+0x14`).
    pub member: Option<Guid>,
    /// His rank (`npc+0xD0`, `0x00791750`).
    pub member_rank: u32,
    /// Specialties (`+0x10`): [`SPECIALTIES`].
    pub specialties: u32,
}

impl CrewSlot {
    pub fn open(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn taken(&self) -> bool {
        self.flags & 2 != 0
    }
}

/// Bits of a crew member's specialties and their names
/// (`$crew_specialty_…`, `0x009630E0`).
pub const SPECIALTIES: [(u32, &str); 6] = [
    (0x04, "demolition"),
    (0x08, "arsonist"),
    (0x10, "safe cracker"),
    (0x20, "engineer"),
    (0x40, "medic"),
    (0x80, "bruiser"),
];

/// Names of the specialties of `bits`.
pub fn specialty_names(bits: u32) -> Vec<&'static str> {
    SPECIALTIES
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| *n)
        .collect()
}

/// Name of a crew rank (`AddPlayerFamilyMember` keeps 3–5).
pub fn rank_name(rank: u32) -> &'static str {
    match rank {
        3 => "soldier",
        4 => "capo",
        5 => "underboss",
        _ => "-",
    }
}

/// Answer to [`crate::Request::Family`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Family {
    /// Every family of the game (`*0x01129904`: `+0x24` list, `+0x28`
    /// count).
    pub families: Vec<FamilyInfo>,
    /// The player's crew.
    pub crew: Vec<CrewSlot>,
}

/// A `Checkpoint` object the game knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Its number (`+0x74`, `cp_737` → 737).
    pub number: u32,
    /// Its chapter (`+0x70`: 60 for the checkpoints of
    /// `chapter60-mission08-killingcastro`).
    pub chapter: u32,
    pub guid: Guid,
}

/// Answer to [`crate::Request::Mission`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Mission {
    /// The current checkpoint (`*0x011299C4 + 0x1C`, `0x008FC6E0`); none
    /// after a start straight into a level.
    pub current: Option<Checkpoint>,
    /// Every checkpoint of the loaded streams (`+0x20` list, `+0x24`
    /// count).
    pub checkpoints: Vec<Checkpoint>,
}

/// A variable of the story (`*0x01223428`: `+0x24` entries of 0x14 bytes,
/// `+0x28` count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variable {
    /// Hash of its `variabledictionary` record (`+0`) — or of its name when
    /// Lua `FindVariable` made it.
    pub hash: u32,
    /// `sdbm(lower(name))` (`+4`, `0x004423F0`); 0 when made by hash.
    pub name_hash: u32,
    pub value: i32,
    /// Kind of the record (`+0xE`): bit 0 — kept in saves (`0x00442550`).
    pub kind: u16,
}

impl Variable {
    /// Whether `hash` names this variable: its name or its record.
    pub fn is(&self, hash: u32) -> bool {
        hash != 0 && (self.name_hash == hash || self.hash == hash)
    }
}

pub(crate) fn put_time(w: &mut Writer, t: &Option<TimeOfDay>) {
    match t {
        Some(t) => {
            w.u8(1);
            w.u32(t.year);
            w.u32(t.day);
            w.u8(t.hour);
            w.u8(t.minute);
            w.u8(t.second);
            w.f32(t.rate);
            w.f32(t.default_rate);
        }
        None => w.u8(0),
    }
}

pub(crate) fn get_time(r: &mut Reader<'_>) -> Result<Option<TimeOfDay>, DecodeError> {
    Ok(if r.bool()? {
        Some(TimeOfDay {
            year: r.u32()?,
            day: r.u32()?,
            hour: r.u8()?,
            minute: r.u8()?,
            second: r.u8()?,
            rate: r.f32()?,
            default_rate: r.f32()?,
        })
    } else {
        None
    })
}

pub(crate) fn put_time_change(w: &mut Writer, c: &TimeChange) {
    match c.time {
        Some(t) => {
            w.u8(1);
            w.bytes(&t);
        }
        None => w.u8(0),
    }
    w.opt_f32(c.rate);
}

pub(crate) fn get_time_change(r: &mut Reader<'_>) -> Result<TimeChange, DecodeError> {
    let time = if r.bool()? {
        let t = [r.u8()?, r.u8()?, r.u8()?];
        if t[0] > 23 || t[1] > 59 || t[2] > 59 {
            return Err(DecodeError::BadValue("time of day"));
        }
        Some(t)
    } else {
        None
    };
    Ok(TimeChange {
        time,
        rate: r.opt_f32()?,
    })
}

pub(crate) fn put_weather(w: &mut Writer, weather: &Weather) {
    match &weather.rain {
        Some(rain) => {
            w.u8(1);
            w.u32(rain.flags);
            w.f32(rain.length);
            w.f32(rain.elapsed);
        }
        None => w.u8(0),
    }
    put_time(w, &weather.time);
}

pub(crate) fn get_weather(r: &mut Reader<'_>) -> Result<Weather, DecodeError> {
    let rain = if r.bool()? {
        Some(Rain {
            flags: r.u32()?,
            length: r.f32()?,
            elapsed: r.f32()?,
        })
    } else {
        None
    };
    Ok(Weather {
        rain,
        time: get_time(r)?,
    })
}

pub(crate) fn put_stream_queue(w: &mut Writer, q: &StreamQueue) {
    w.bool(q.busy);
    w.u32(q.requests.len() as u32);
    for s in &q.requests {
        w.str(&s.name);
        w.u8(s.action as u8);
        w.u32(s.id);
        w.u32(s.state);
        w.bool(s.done);
        w.u32(s.frames);
        w.bool(s.expired);
    }
}

pub(crate) fn get_stream_queue(r: &mut Reader<'_>) -> Result<StreamQueue, DecodeError> {
    let busy = r.bool()?;
    let n = r.count(16)?;
    let mut requests = Vec::with_capacity(n);
    for _ in 0..n {
        requests.push(StreamRequest {
            name: r.str()?,
            action: match r.u8()? {
                0 => StreamAction::Load,
                1 => StreamAction::Unload,
                _ => return Err(DecodeError::BadValue("stream action")),
            },
            id: r.u32()?,
            state: r.u32()?,
            done: r.bool()?,
            frames: r.u32()?,
            expired: r.bool()?,
        });
    }
    Ok(StreamQueue { busy, requests })
}

pub(crate) fn put_money_change(w: &mut Writer, c: &MoneyChange) {
    match c {
        MoneyChange::Add(v) => {
            w.u8(0);
            w.f32(*v);
        }
        MoneyChange::Set(v) => {
            w.u8(1);
            w.f32(*v);
        }
    }
}

pub(crate) fn get_money_change(r: &mut Reader<'_>) -> Result<MoneyChange, DecodeError> {
    let kind = r.u8()?;
    let v = r.f32()?;
    if !v.is_finite() {
        return Err(DecodeError::BadValue("money"));
    }
    match kind {
        0 => Ok(MoneyChange::Add(v)),
        1 => Ok(MoneyChange::Set(v)),
        _ => Err(DecodeError::BadValue("money change")),
    }
}

pub(crate) fn put_money(w: &mut Writer, m: &Option<Money>) {
    match m {
        Some(m) => {
            w.u8(1);
            w.f32(m.cash);
            w.u32(m.books.len() as u32);
            for b in &m.books {
                w.f32(*b);
            }
        }
        None => w.u8(0),
    }
}

pub(crate) fn get_money(r: &mut Reader<'_>) -> Result<Option<Money>, DecodeError> {
    if !r.bool()? {
        return Ok(None);
    }
    let cash = r.f32()?;
    let n = r.count(4)?;
    let mut books = Vec::with_capacity(n);
    for _ in 0..n {
        books.push(r.f32()?);
    }
    Ok(Some(Money { cash, books }))
}

pub(crate) fn put_family(w: &mut Writer, f: &Family) {
    w.u32(f.families.len() as u32);
    for x in &f.families {
        w.u32(x.hash);
        w.f32(x.cash);
    }
    w.u32(f.crew.len() as u32);
    for s in &f.crew {
        w.u8(s.slot);
        w.u32(s.flags);
        w.u32(s.rank);
        match &s.member {
            Some(g) => {
                w.u8(1);
                w.bytes(g);
            }
            None => w.u8(0),
        }
        w.u32(s.member_rank);
        w.u32(s.specialties);
    }
}

pub(crate) fn get_family(r: &mut Reader<'_>) -> Result<Family, DecodeError> {
    let n = r.count(8)?;
    let mut families = Vec::with_capacity(n);
    for _ in 0..n {
        families.push(FamilyInfo {
            hash: r.u32()?,
            cash: r.f32()?,
        });
    }
    let n = r.count(18)?;
    let mut crew = Vec::with_capacity(n);
    for _ in 0..n {
        crew.push(CrewSlot {
            slot: r.u8()?,
            flags: r.u32()?,
            rank: r.u32()?,
            member: if r.bool()? { Some(r.guid()?) } else { None },
            member_rank: r.u32()?,
            specialties: r.u32()?,
        });
    }
    Ok(Family { families, crew })
}

fn put_checkpoint(w: &mut Writer, c: &Checkpoint) {
    w.u32(c.number);
    w.u32(c.chapter);
    w.bytes(&c.guid);
}

fn get_checkpoint(r: &mut Reader<'_>) -> Result<Checkpoint, DecodeError> {
    Ok(Checkpoint {
        number: r.u32()?,
        chapter: r.u32()?,
        guid: r.guid()?,
    })
}

pub(crate) fn put_mission(w: &mut Writer, m: &Mission) {
    match &m.current {
        Some(c) => {
            w.u8(1);
            put_checkpoint(w, c);
        }
        None => w.u8(0),
    }
    w.u32(m.checkpoints.len() as u32);
    for c in &m.checkpoints {
        put_checkpoint(w, c);
    }
}

pub(crate) fn get_mission(r: &mut Reader<'_>) -> Result<Mission, DecodeError> {
    let current = if r.bool()? {
        Some(get_checkpoint(r)?)
    } else {
        None
    };
    let n = r.count(24)?;
    let mut checkpoints = Vec::with_capacity(n);
    for _ in 0..n {
        checkpoints.push(get_checkpoint(r)?);
    }
    Ok(Mission {
        current,
        checkpoints,
    })
}

pub(crate) fn put_variables(w: &mut Writer, list: &[Variable]) {
    w.u32(list.len() as u32);
    for v in list {
        w.u32(v.hash);
        w.u32(v.name_hash);
        w.u32(v.value as u32);
        w.u16(v.kind);
    }
}

pub(crate) fn get_variables(r: &mut Reader<'_>) -> Result<Vec<Variable>, DecodeError> {
    let n = r.count(14)?;
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        list.push(Variable {
            hash: r.u32()?,
            name_hash: r.u32()?,
            value: r.u32()? as i32,
            kind: r.u16()?,
        });
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_names_as_the_game_spells_them() {
        assert_eq!(stream_name("gf2/cuba/local.str"), "gf2\\cuba\\local");
        assert_eq!(stream_name(" gf2\\cuba\\local "), "gf2\\cuba\\local");
        assert_eq!(stream_name("GF2/Cuba.STR"), "GF2\\Cuba");
        assert_eq!(stream_name("str"), "str");
    }

    #[test]
    fn specialties_and_ranks() {
        assert_eq!(specialty_names(0x04 | 0x40), ["demolition", "medic"]);
        assert!(specialty_names(0x03).is_empty());
        assert_eq!(rank_name(4), "capo");
        assert_eq!(rank_name(9), "-");
        let slot = CrewSlot {
            slot: 0,
            flags: 3,
            rank: 3,
            member: None,
            member_rank: 0,
            specialties: 0,
        };
        assert!(slot.open() && slot.taken());
    }

    #[test]
    fn variables_by_name_or_record() {
        let v = Variable {
            hash: 0x001A_01D2,
            name_hash: crate::name_hash("global_tutorial_payday"),
            value: 1,
            kind: 1,
        };
        // The pair read from the game on the stand (§13.6 of live-link.md).
        assert_eq!(v.name_hash, 0x9381_3AB9);
        assert!(v.is(0x9381_3AB9) && v.is(0x001A_01D2) && !v.is(0) && !v.is(5));
    }
}
