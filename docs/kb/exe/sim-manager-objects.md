---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 49]
sources: ["docs/analysis/live-link.md §4", "docs/analysis/live-link.md §11.5", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "crates/doctor-toolkit-live-asi/src/game.rs"]
tags: [exe, simgroup, objects]
updated: 2026-11-16
---
# SimManager: объекты, группы, приведение класса

Адреса — виртуальные, издание PC 1.0.789.0. SimManager держит все объекты SimGroup: таблицу по GUID и таблицу групп. Номер группы объекта — идентификатор потока, который его создал. Класс объекта узнаётся виртуальным методом `As`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x01223410` | данные: SimManager (указатель); `+0x48` корзины и `+0x4C` число корзин таблицы групп, `+0x80`/`+0x84` — таблица GUID | live-link.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0046C590` | конструктор `Base`: регистрирует объект, ставит флаги слушателя `1 \| 0x12` | live-link.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0046C523` | место конструктора `Base` с адресом SimManager `0x01223410` | crates/doctor-toolkit-live/src/edition.rs |
| `0x004467C0` | SimManager: регистрация объекта (группа `+0x1C`, GUID `+0x20`, таблица `+0x48`); ставит флаг `0x20000000` в `+0x14` | live-link.md §4; crates/doctor-toolkit-live/src/addr.rs; crates/doctor-toolkit-live/src/edition.rs |
| `0x00445FF0` | SimManager: объект по GUID | live-link.md §4; crates/doctor-toolkit-live/src/edition.rs |
| `0x00446E30` | создание объекта из записи SimGroup | live-link.md §11.5 |
| `0x00446E9D` | место, где группа объекта берётся из загружаемого потока `*(StreamManager+0x84)+0x1C` | live-link.md §11.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0046D450` | `Entity::As` — виртуальный `+0x10` `bool As(хэш, void** out)` | live-link.md §4; crates/doctor-toolkit-live/src/edition.rs |
| `0x007DCCD0` | `As` класса `PlayerTeleportRelay` | live-link.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0046C460` | `Base::As`: ничего не сравнивает (у классов без своего `As` класс модулю не известен) | live-link.md §11.5 |
| `0x00D55000`, `0x00E50000` | данные: границы диапазона, в котором модуль ожидает таблицы методов объектов и частей | crates/doctor-toolkit-live-asi/src/game.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §4 «Объекты и потоки», §11.5 «Все объекты»
