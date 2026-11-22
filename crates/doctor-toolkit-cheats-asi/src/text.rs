//! The texts of the menu in English and Russian, and the language of the
//! game.

use std::path::{Path, PathBuf};

/// The languages of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    /// The language of a locale code of the game (`ru`, `en`, `es`…):
    /// Russian for `ru`, English for anything else.
    pub fn of_locale(code: &str) -> Self {
        if code.trim().to_ascii_lowercase().starts_with("ru") {
            Lang::Ru
        } else {
            Lang::En
        }
    }
}

macro_rules! texts {
    ($($name:ident => $en:expr, $ru:expr;)*) => {
        /// A text of the menu.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum T { $($name),* }

        impl T {
            pub fn get(self, lang: Lang) -> &'static str {
                match (self, lang) {
                    $((T::$name, Lang::En) => $en, (T::$name, Lang::Ru) => $ru,)*
                }
            }
        }

        #[cfg(test)]
        pub const ALL: &[T] = &[$(T::$name),*];
    };
}

texts! {
    Title => "DEVELOPER MENU", "МЕНЮ РАЗРАБОТЧИКА";
    Player => "Player", "Игрок";
    Weapons => "Weapons", "Оружие";
    Crew => "Crew", "Команда";
    Families => "Families", "Семьи";
    Money => "Money +$10 000", "Деньги +$10 000";
    GodMode => "God mode", "Бессмертие";
    OneHit => "One hit and down (OHKO)", "Один удар — и всё (OHKO)";
    Teleport => "Teleport to the waypoint", "Телепорт к метке";
    Hud => "Interface on screen", "Интерфейс на экране";
    Hour => "Time of day, hour", "Время суток, час";
    InfiniteAmmo => "Infinite ammunition", "Бесконечные патроны";
    NoReload => "No reload", "Без перезарядки";
    Damage => "Damage of my weapon, ×", "Урон моего оружия, ×";
    FireRate => "Rate of fire, ×", "Скорострельность, ×";
    Spread => "Spread, ×", "Разброс, ×";
    NoRecoil => "No camera shake", "Без отдачи камеры";
    NpcDamage => "Damage of enemies, ×", "Урон противников, ×";
    GiveWeapon => "Give a weapon", "Выдать оружие";
    CrewPlace => "Place in the tree", "Место в древе";
    Demolition => "Demolition", "Подрывник";
    Arsonist => "Arsonist", "Поджигатель";
    SafeCracker => "Safe cracker", "Медвежатник";
    Engineer => "Engineer", "Инженер";
    Medic => "Medic", "Медик";
    Bruiser => "Bruiser", "Громила";
    CrewLicence => "Weapon licence", "Оружейная лицензия";
    CrewSlot => "Open this place", "Открыть это место";
    CrewEscort => "Men with me", "Людей со мной";
    Soldier => "soldier", "солдат";
    Capo => "capo", "капо";
    Underboss => "underboss", "младший босс";
    PlaceClosed => "closed", "закрыто";
    PlaceFree => "free", "свободно";
    NoMember => "No man in this place", "На этом месте никого нет";
    PlaceOpened => "The place is open", "Место открыто";
    PlaceWasOpen => "The place is already open", "Место уже открыто";
    LicenceSet => "Licence: ", "Лицензия: ";
    NoLicence => "The game keeps no licence for him yet", "У игры ещё нет его лицензии";
    SpecialtiesLater => "Specialties set: he takes them when he comes again", "Специальности записаны: он получит их, когда придёт снова";
    NoEscort => "No escort here", "Здесь нет свиты";
    Vendetta => "Vendetta between families", "Вендетта между семьями";
    Favors => "Favours", "Услуги";
    Owner => "Owner of the chosen business", "Владелец выбранного заведения";
    AllRackets => "All rackets to one family", "Все рэкеты одной семье";
    MemberStatus => "Status of the chosen rival", "Статус выбранного противника";
    Strategy => "Strategy of the families", "Стратегия семей";
    Compounds => "Family of the compounds", "Семья на укреплениях";
    On => "ON", "ВКЛ";
    Off => "OFF", "ВЫКЛ";
    Soon => "soon", "скоро";
    Hint => "Up, Down — choose · Left, Right — change · Enter — do · Esc — back", "Вверх, вниз — выбор · влево, вправо — значение · Enter — выполнить · Esc — назад";
    SavesNote => "• goes into the save", "• попадает в сохранение";
    NotYet => "Not in this version yet", "В этой версии ещё нет";
    NotHere => "Not available here", "Здесь недоступно";
    MoneyDone => "Money: $", "Деньги: $";
    Coords => "Coordinates", "Координаты";
    NoWaypoint => "No waypoint: put one in Don's view", "Метки нет: поставьте её во «Взгляде дона»";
    TeleportLater => "Teleport when the game goes on", "Телепорт — когда игра продолжится";
    Teleported => "Teleported to the waypoint", "Телепорт к метке выполнен";
    NoTeleport => "The game cannot move the player now", "Сейчас игра не может переместить игрока";
    TeleportWait => "Waiting for the ground under the waypoint to load", "Жду, пока загрузится земля под меткой";
    NoGround => "No ground under the waypoint: back where you were", "Земли под меткой нет: вы там, где были";
    Water => "Water under the waypoint: no teleport", "Под меткой вода: телепорта не будет";
    HourSet => "Time of day: ", "Время суток: ";
    Remember => "Remember the settings", "Запомнить настройки";
    Remembered => "Settings kept in doctor_cheats.cfg", "Настройки сохранены в doctor_cheats.cfg";
    NotRemembered => "Could not write doctor_cheats.cfg", "Не удалось записать doctor_cheats.cfg";
}

/// The language the menu follows: `language` of the settings, or the
/// game's (`Locale = xx` of `settings.txt`, then the registry, as the game
/// does — `docs/unified-game.md`), English when none is found.
pub fn language(setting: Option<Lang>, settings_txt: &[PathBuf], registry: Option<String>) -> Lang {
    if let Some(lang) = setting {
        return lang;
    }
    for path in settings_txt {
        if let Some(code) = std::fs::read_to_string(path)
            .ok()
            .as_deref()
            .and_then(locale_line)
        {
            return Lang::of_locale(&code);
        }
    }
    registry.map_or(Lang::En, |code| Lang::of_locale(&code))
}

/// `Locale = xx` of a `settings.txt` of the game.
pub fn locale_line(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("locale")
            .then(|| value.trim().to_owned())
            .filter(|v| !v.is_empty())
    })
}

/// The `settings.txt` files of the game: `%LOCALAPPDATA%\EA Games\*\`.
pub fn settings_files(local_app_data: &Path) -> Vec<PathBuf> {
    let Ok(titles) = std::fs::read_dir(local_app_data.join("EA Games")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = titles
        .flatten()
        .map(|e| e.path().join("settings.txt"))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_has_both_languages() {
        for t in ALL {
            assert!(!t.get(Lang::En).is_empty(), "{t:?}");
            assert!(!t.get(Lang::Ru).is_empty(), "{t:?}");
        }
    }

    #[test]
    fn the_language_follows_the_setting_then_the_game() {
        assert_eq!(locale_line("Foo = 1\nLocale = ru\n").as_deref(), Some("ru"));
        assert_eq!(locale_line("locale=en"), Some("en".into()));
        assert_eq!(locale_line("Locale = \n"), None);
        assert_eq!(Lang::of_locale("RU"), Lang::Ru);
        assert_eq!(Lang::of_locale("cz"), Lang::En);
        let dir = std::env::temp_dir().join(format!("doctor-cheats-lang-{}", std::process::id()));
        let file = dir.join("EA Games/The Godfather II/settings.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "Locale = ru\r\n").unwrap();
        let files = settings_files(&dir);
        assert_eq!(files, vec![file]);
        assert_eq!(language(None, &files, Some("en".into())), Lang::Ru);
        assert_eq!(language(Some(Lang::En), &files, None), Lang::En);
        assert_eq!(language(None, &[], Some("ru".into())), Lang::Ru);
        assert_eq!(language(None, &[], None), Lang::En);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
