//! The menu: pages of items, the cursor, what a key does. No game here —
//! the mod turns the [`Effect`]s into actions on the main thread.

use crate::text::T;

/// What an item stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum What {
    Page(usize),
    Money,
    GodMode,
    OneHit,
    Teleport,
    Hud,
    Hour,
    InfiniteAmmo,
    NoReload,
    Damage,
    FireRate,
    Spread,
    NoRecoil,
    NpcDamage,
    GiveWeapon,
    CrewSkills,
    CrewLicence,
    CrewSlot,
    CrewEscort,
    Vendetta,
    Favors,
    Owner,
    AllRackets,
    MemberStatus,
    Strategy,
    Compounds,
    Coords,
    Remember,
}

/// How an item behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Opens a page.
    Page,
    /// Does something once (Enter).
    Action,
    /// On or off (Enter, Left, Right).
    Toggle(bool),
    /// A value (Left, Right).
    Number {
        value: f32,
        min: f32,
        max: f32,
        step: f32,
    },
    /// Shows something of the game (the mod gives the text).
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub text: T,
    pub what: What,
    pub kind: Kind,
    /// Works in this version; otherwise shown dimmed, "soon".
    pub ready: bool,
    /// What it changes goes into the save.
    pub saves: bool,
}

pub struct Page {
    pub title: T,
    pub items: Vec<Item>,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    None,
    /// The cursor or the page changed: nothing for the game.
    Moved,
    /// Close the menu.
    Close,
    /// Do the action of an item.
    Run(What),
    /// The value of an item changed (see [`Menu::item`]).
    Changed(What),
    /// The item is not in this version.
    NotYet,
}

/// Keys of the menu (from the keyboard or the gamepad).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Back,
}

const fn item(text: T, what: What, kind: Kind, ready: bool, saves: bool) -> Item {
    Item {
        text,
        what,
        kind,
        ready,
        saves,
    }
}

const fn number(value: f32, min: f32, max: f32, step: f32) -> Kind {
    Kind::Number {
        value,
        min,
        max,
        step,
    }
}

/// The pages of this version: what works is `ready`; the rest is there to
/// show what is coming (stages 120–124 of `PLAN-cheats.md`).
pub fn pages() -> Vec<Page> {
    use Kind::{Action, Page as Open, Toggle};
    use What::*;
    vec![
        self::Page {
            title: T::Title,
            items: vec![
                item(T::Player, Page(1), Open, true, false),
                item(T::Weapons, Page(2), Open, true, false),
                item(T::Crew, Page(3), Open, true, false),
                item(T::Families, Page(4), Open, true, false),
                item(T::Remember, Remember, Toggle(false), true, false),
            ],
        },
        self::Page {
            title: T::Player,
            items: vec![
                item(T::Money, Money, Action, true, true),
                item(T::GodMode, GodMode, Toggle(false), true, false),
                item(T::OneHit, OneHit, Toggle(false), true, false),
                // The waypoint has no height (y = 0): waits for the ground
                // under it from the game's physics (stage 120b).
                item(T::Teleport, Teleport, Action, false, false),
                item(T::Hud, Hud, Toggle(true), true, false),
                item(T::Hour, Hour, number(12.0, 0.0, 23.0, 1.0), true, true),
                item(T::Coords, Coords, Kind::Info, true, false),
            ],
        },
        self::Page {
            title: T::Weapons,
            items: vec![
                item(T::InfiniteAmmo, InfiniteAmmo, Toggle(false), true, false),
                item(T::NoReload, NoReload, Toggle(false), true, false),
                item(T::Damage, Damage, number(1.0, 0.5, 10.0, 0.5), true, false),
                item(
                    T::FireRate,
                    FireRate,
                    number(1.0, 0.5, 5.0, 0.5),
                    true,
                    false,
                ),
                item(T::Spread, Spread, number(1.0, 0.0, 2.0, 0.25), true, false),
                item(T::NoRecoil, NoRecoil, Toggle(false), true, false),
                item(
                    T::NpcDamage,
                    NpcDamage,
                    number(1.0, 0.0, 5.0, 0.25),
                    true,
                    false,
                ),
                item(T::GiveWeapon, GiveWeapon, Action, false, true),
            ],
        },
        self::Page {
            title: T::Crew,
            items: vec![
                item(T::CrewSkills, CrewSkills, Action, false, true),
                item(T::CrewLicence, CrewLicence, Action, false, true),
                item(T::CrewSlot, CrewSlot, Action, false, true),
                item(
                    T::CrewEscort,
                    CrewEscort,
                    number(3.0, 1.0, 7.0, 1.0),
                    false,
                    false,
                ),
            ],
        },
        self::Page {
            title: T::Families,
            items: vec![
                item(T::Vendetta, Vendetta, Action, false, true),
                item(T::Favors, Favors, Action, false, true),
                item(T::Owner, Owner, Action, false, true),
                item(T::AllRackets, AllRackets, Action, false, true),
                item(T::MemberStatus, MemberStatus, Action, false, true),
                item(T::Strategy, Strategy, Action, false, false),
                item(T::Compounds, Compounds, Action, false, true),
            ],
        },
    ]
}

/// The items whose values may be kept between starts, by name in
/// `doctor_cheats.cfg` (the hour and the actions are not).
pub fn remembered() -> &'static [(&'static str, What)] {
    &[
        ("god", What::GodMode),
        ("one_hit", What::OneHit),
        ("hud", What::Hud),
        ("ammo", What::InfiniteAmmo),
        ("no_reload", What::NoReload),
        ("damage", What::Damage),
        ("fire_rate", What::FireRate),
        ("spread", What::Spread),
        ("no_recoil", What::NoRecoil),
        ("enemy_damage", What::NpcDamage),
    ]
}

/// The menu and where the player is in it.
pub struct Menu {
    pub pages: Vec<Page>,
    /// Pages opened, with the cursor on each; the first is the main page.
    pub stack: Vec<(usize, usize)>,
}

impl Default for Menu {
    fn default() -> Self {
        Self::new(pages())
    }
}

impl Menu {
    pub fn new(pages: Vec<Page>) -> Self {
        Self {
            pages,
            stack: vec![(0, 0)],
        }
    }

    /// The page shown and the cursor on it.
    pub fn here(&self) -> (&Page, usize) {
        let (page, cursor) = *self.stack.last().unwrap_or(&(0, 0));
        (&self.pages[page], cursor)
    }

    /// The item with this meaning, wherever it is.
    pub fn item(&self, what: What) -> Option<&Item> {
        self.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .find(|i| i.what == what)
    }

    fn item_mut(&mut self, what: What) -> Option<&mut Item> {
        self.pages
            .iter_mut()
            .flat_map(|p| p.items.iter_mut())
            .find(|i| i.what == what)
    }

    /// Whether a toggle is on.
    pub fn on(&self, what: What) -> bool {
        matches!(self.item(what).map(|i| i.kind), Some(Kind::Toggle(true)))
    }

    /// The value of a number item.
    pub fn value(&self, what: What) -> Option<f32> {
        match self.item(what)?.kind {
            Kind::Number { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Sets a number item (clamped), e.g. to what the game has.
    pub fn set_value(&mut self, what: What, to: f32) {
        if let Some(Item {
            kind: Kind::Number {
                value, min, max, ..
            },
            ..
        }) = self.item_mut(what)
        {
            *value = to.clamp(*min, *max);
        }
    }

    /// Sets a toggle.
    pub fn set_toggle(&mut self, what: What, on: bool) {
        if let Some(i) = self.item_mut(what)
            && matches!(i.kind, Kind::Toggle(_))
        {
            i.kind = Kind::Toggle(on);
        }
    }

    /// Marks an item as working or not (a hook that could not be made).
    pub fn set_ready(&mut self, what: What, ready: bool) {
        if let Some(i) = self.item_mut(what) {
            i.ready = ready;
        }
    }

    /// The values to keep: `(name, "on"|"off"|number)`.
    pub fn values(&self) -> Vec<(String, String)> {
        remembered()
            .iter()
            .filter_map(|&(name, what)| {
                let v = match self.item(what)?.kind {
                    Kind::Toggle(on) => if on { "on" } else { "off" }.to_owned(),
                    Kind::Number { value, .. } => format!("{value}"),
                    _ => return None,
                };
                Some((name.to_owned(), v))
            })
            .collect()
    }

    /// Puts kept values back; unknown names and bad numbers are left.
    pub fn restore(&mut self, values: &[(String, String)]) {
        for (name, value) in values {
            let Some(&(_, what)) = remembered().iter().find(|(n, _)| n == name) else {
                continue;
            };
            let toggle = match self.item(what).map(|i| i.kind) {
                Some(Kind::Toggle(_)) => true,
                Some(Kind::Number { .. }) => false,
                _ => continue,
            };
            if toggle {
                if let Some(item) = self.item_mut(what) {
                    item.kind = Kind::Toggle(value == "on");
                }
            } else if let Ok(v) = value.parse::<f32>() {
                self.set_value(what, v);
            }
        }
    }

    /// Back to the main page (when the menu opens again).
    pub fn reset(&mut self) {
        self.stack.truncate(1);
    }

    pub fn press(&mut self, key: Key) -> Effect {
        let Some(&mut (page, ref mut cursor)) = self.stack.last_mut() else {
            return Effect::None;
        };
        let n = self.pages[page].items.len();
        if n == 0 {
            return Effect::None;
        }
        match key {
            Key::Up => {
                *cursor = (*cursor + n - 1) % n;
                Effect::Moved
            }
            Key::Down => {
                *cursor = (*cursor + 1) % n;
                Effect::Moved
            }
            Key::Back => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    Effect::Moved
                } else {
                    Effect::Close
                }
            }
            Key::Enter | Key::Left | Key::Right => {
                let cursor = *cursor;
                let item = &mut self.pages[page].items[cursor];
                match (item.kind, key) {
                    (Kind::Page, Key::Enter) => {
                        if let What::Page(to) = item.what {
                            self.stack.push((to, 0));
                            Effect::Moved
                        } else {
                            Effect::None
                        }
                    }
                    (Kind::Page, _) | (Kind::Info, _) => Effect::None,
                    _ if !item.ready => Effect::NotYet,
                    (Kind::Action, Key::Enter) => Effect::Run(item.what),
                    (Kind::Action, _) => Effect::None,
                    (Kind::Toggle(on), _) => {
                        item.kind = Kind::Toggle(!on);
                        Effect::Changed(item.what)
                    }
                    (Kind::Number { .. }, Key::Enter) => Effect::None,
                    (
                        Kind::Number {
                            value,
                            min,
                            max,
                            step,
                        },
                        _,
                    ) => {
                        let to = if key == Key::Left {
                            value - step
                        } else {
                            value + step
                        }
                        .clamp(min, max);
                        if to == value {
                            return Effect::None;
                        }
                        item.kind = Kind::Number {
                            value: to,
                            min,
                            max,
                            step,
                        };
                        Effect::Changed(item.what)
                    }
                }
            }
        }
    }
}

/// Turns keys held down into presses: one when pressed, then, for the
/// arrows, again every [`Repeat::EVERY`] ms after [`Repeat::DELAY`] ms.
#[derive(Default)]
pub struct Repeat {
    held: Vec<(Key, u32)>,
}

impl Repeat {
    pub const DELAY: u32 = 400;
    pub const EVERY: u32 = 70;

    /// The presses of this tick: `down` — the keys held now, `now` — ms.
    pub fn feed(&mut self, down: &[Key], now: u32) -> Vec<Key> {
        let mut out = Vec::new();
        self.held.retain(|(k, _)| down.contains(k));
        for &key in down {
            match self.held.iter_mut().find(|(k, _)| *k == key) {
                None => {
                    self.held.push((key, now.wrapping_add(Self::DELAY)));
                    out.push(key);
                }
                Some((_, next)) => {
                    let arrow = matches!(key, Key::Up | Key::Down | Key::Left | Key::Right);
                    if arrow && now.wrapping_sub(*next) < u32::MAX / 2 {
                        *next = now.wrapping_add(Self::EVERY);
                        out.push(key);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_open_and_close_and_the_cursor_wraps() {
        let mut m = Menu::default();
        assert_eq!(m.here().0.title, T::Title);
        assert_eq!(m.press(Key::Up), Effect::Moved);
        assert_eq!(m.here().1, 4);
        assert_eq!(m.press(Key::Down), Effect::Moved);
        assert_eq!(m.press(Key::Enter), Effect::Moved);
        assert_eq!(m.here().0.title, T::Player);
        assert_eq!(m.press(Key::Enter), Effect::Run(What::Money));
        assert_eq!(m.press(Key::Back), Effect::Moved);
        assert_eq!(m.press(Key::Back), Effect::Close);
        m.press(Key::Down);
        m.press(Key::Enter);
        assert_eq!(m.here().0.title, T::Weapons);
        m.reset();
        assert_eq!(m.here().0.title, T::Title);
        assert_eq!(m.here().1, 1, "the cursor of the main page is kept");
    }

    #[test]
    fn items_not_ready_say_so_and_values_stay_in_range() {
        let mut pages = pages();
        // OHKO is made "not ready" for the test.
        pages[1].items[2].ready = false;
        let mut m = Menu::new(pages);
        m.press(Key::Enter); // Player
        m.press(Key::Down);
        assert_eq!(m.press(Key::Enter), Effect::Changed(What::GodMode));
        assert_eq!(m.item(What::GodMode).unwrap().kind, Kind::Toggle(true));
        m.press(Key::Down);
        assert_eq!(m.press(Key::Enter), Effect::NotYet);
        m.press(Key::Down);
        m.press(Key::Down);
        m.press(Key::Down);
        for _ in 0..30 {
            m.press(Key::Right);
        }
        assert!(
            matches!(m.item(What::Hour).unwrap().kind, Kind::Number { value, .. } if value == 23.0)
        );
        assert_eq!(m.press(Key::Right), Effect::None);
        assert_eq!(m.press(Key::Left), Effect::Changed(What::Hour));
        assert_eq!(m.value(What::Hour), Some(22.0));
        m.set_value(What::Hour, 40.0);
        assert_eq!(m.value(What::Hour), Some(23.0));
        assert!(m.on(What::GodMode));
        assert!(!m.on(What::OneHit));
        m.press(Key::Down);
        assert_eq!(m.here().0.items[m.here().1].what, What::Coords);
        assert_eq!(m.press(Key::Enter), Effect::None);
    }

    #[test]
    fn every_item_of_a_page_is_unique_and_pages_exist() {
        let pages = pages();
        let mut seen = std::collections::HashSet::new();
        for p in &pages {
            for i in &p.items {
                assert!(seen.insert(i.what), "{:?}", i.what);
                if let What::Page(to) = i.what {
                    assert!(to < pages.len());
                }
            }
        }
    }

    #[test]
    fn values_are_kept_and_put_back() {
        let mut m = Menu::default();
        m.set_value(What::Damage, 2.5);
        let v = m.values();
        assert!(v.contains(&("damage".into(), "2.5".into())));
        assert!(v.contains(&("god".into(), "off".into())));
        assert!(v.contains(&("hud".into(), "on".into())));
        let mut n = Menu::default();
        n.restore(&[
            ("god".into(), "on".into()),
            ("damage".into(), "99".into()),
            ("spread".into(), "x".into()),
            ("nothing".into(), "on".into()),
        ]);
        assert!(n.on(What::GodMode));
        assert_eq!(n.value(What::Damage), Some(10.0));
        assert_eq!(n.value(What::Spread), Some(1.0));
    }

    #[test]
    fn held_arrows_repeat_and_other_keys_do_not() {
        let mut r = Repeat::default();
        assert_eq!(
            r.feed(&[Key::Down, Key::Enter], 1000),
            vec![Key::Down, Key::Enter]
        );
        assert!(r.feed(&[Key::Down, Key::Enter], 1200).is_empty());
        assert_eq!(r.feed(&[Key::Down, Key::Enter], 1400), vec![Key::Down]);
        assert!(r.feed(&[Key::Down], 1450).is_empty());
        assert_eq!(r.feed(&[Key::Down], 1470), vec![Key::Down]);
        assert!(r.feed(&[], 1500).is_empty());
        assert_eq!(r.feed(&[Key::Down], 1510), vec![Key::Down]);
    }
}
