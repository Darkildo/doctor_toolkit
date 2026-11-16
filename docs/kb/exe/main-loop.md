---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 32, 58]
sources: ["docs/analysis/live-link.md §2", "docs/analysis/launch-flags.md §3.1", "docs/analysis/launch-flags.md §3.4", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, main-loop, hooks]
updated: 2026-11-16
---
# Главный цикл и точки перехвата кадра

Адреса — виртуальные, издание PC 1.0.789.0. Главный цикл `godfather2.exe` крутит диспетчер трёх состояний (`InitOnceMainLoop`, `FrontendMainLoop`, `GameMainLoop`). Модуль живой связи ставит два перехвата: вызов в начале каждого прохода цикла (чтение) и слот обновления `GameMainLoop` (запись внутри кадра).

| Адрес | Что это | Источник |
|---|---|---|
| `0x0069D9D0` | `WinMain`: зовёт главный цикл; `lpCmdLine` не используется | live-link.md §2; launch-flags.md §2 |
| `0x0068AEB0` | главный цикл: на каждом проходе `0x00439FA0()`, затем кадр текущего состояния | live-link.md §2 |
| `0x0068B04B` | `call 0x00439FA0` (`E8 50 EF DA FF`), раз за проход цикла; первая точка перехвата модуля (чтение) | live-link.md §2 |
| `0x00439FA0` | функция, вызываемая раз за проход цикла: `cdecl`, без аргументов, `bool` в `AL` | live-link.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00683450` | кадр текущего состояния: метод `+0xC` состояния; 2 — сменить состояние | live-link.md §2 |
| `0x0068AF30` | место главного цикла с диспетчером `0x0112980C` (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x0112980C` | данные: диспетчер состояний (указатель), `+4` — текущее состояние | live-link.md §2; launch-flags.md §3.1 |
| `0x00D5A1F8` | данные: таблица методов `InitOnceMainLoop` | live-link.md §2; launch-flags.md §3.1 |
| `0x00D57758` | данные: таблица методов `FrontendMainLoop` | live-link.md §2 |
| `0x00D57728` | данные: таблица методов `GameMainLoop` | live-link.md §2 |
| `0x0067C390` | конструктор `GameMainLoop` | live-link.md §2; crates/doctor-toolkit-live/src/edition.rs |
| `0x0067C39B` | место конструктора `GameMainLoop`, где пишется таблица `0x00D57728` | crates/doctor-toolkit-live/src/edition.rs |
| `0x0067C590` | конструктор `FrontendMainLoop` | live-link.md §2; crates/doctor-toolkit-live/src/edition.rs |
| `0x0067C59B` | место конструктора `FrontendMainLoop`, где пишется таблица `0x00D57758` | crates/doctor-toolkit-live/src/edition.rs |
| `0x0067C290` | `GameMainLoop` `+0xC`: начало кадра `0x0068BAF0(1)` → метод `+0x2C` (обновление) → конец `0x0068BCD0(1)` | live-link.md §2 |
| `0x00D57754` | данные: слот `+0x2C` таблицы `GameMainLoop` (обновление); вторая точка перехвата модуля (запись внутри кадра) | live-link.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0067D160` | обновление `GameMainLoop` (то, что лежит в слоте `0x00D57754`) | live-link.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0067C560` | метод слота `+0x28` `GameMainLoop`: выход из состояния, в игре не вызывается | live-link.md §2 |
| `0x0068BBE0` | из обновления: рассылает такты и зовёт отрисовку уровня `0x0040E1F0` | live-link.md §12.2 |
| `0x0040EBB0` | рассылка тактов (из `0x0068BBE0`) | live-link.md §12.2 |
| `0x0068BC75`, `0x0068BC8A` | места вызова отрисовки уровня `0x0040E1F0` в `0x0068BBE0` | live-link.md §12.2 |
| `0x00683280` | диспетчер: имя состояния → номер | launch-flags.md §3.1 |
| `0x006833D0` | диспетчер: номер → состояние (метод `+0x14` — следующее состояние) | launch-flags.md §3.1 |
| `0x0067CC50` | вход `InitOnceMainLoop` (там же разбор флагов `cmd.txt`) | launch-flags.md §3.1 |
| `0x0067C7F0` | вход `FrontendMainLoop`: грузит `GF2\frontend.str` | launch-flags.md §3.1 |
| `0x0067D140` | вход `GameMainLoop` (→ `0x0067D090`) | launch-flags.md §3.1 |
| `0x0067D090` | вход `GameMainLoop`: подготовка загрузки (`0x0067E210`, `0x006817C0`, `0x004E0180`, встроенный поток через `0x009C7700`) | launch-flags.md §3.4 |
| `0x0067E210`, `0x006817C0`, `0x004E0180` | вызовы подготовки загрузки при входе в `GameMainLoop` | launch-flags.md §3.4 |
| `0x009C7700` | загрузка встроенного потока `embedded:\gf2_patch_text_str` при входе в `GameMainLoop` | launch-flags.md §3.4 |
| `0x0067C9F0` | `(папка, список)`: к папке добавляет `\`, режет список на слова и для каждого зовёт `0x00411C70` | launch-flags.md §3.1 |
| `0x00411C70` | на каждое слово списка: сообщение `0x01206A28` и запрос потока `0x00403A50(ПАПКА\ИМЯ, …)` | launch-flags.md §3.1 |
| `0x01206A28` | данные: сообщение, посылаемое при запросе потока из `0x00411C70` | launch-flags.md §3.1 |
| `0x00412740` | поиск папки с `#` в начале среди зарегистрированных записей (не проверялось) | launch-flags.md §3.1 |
| `0x00930E70` | старт из меню: кладёт папку и список во `FrontendMainLoop`, задаёт глобальный GUID точки появления | launch-flags.md §3.1; launch-flags.md §3.2 |
| `0x0067C360` | `FrontendMainLoop`: приём папки и списка от `0x00930E70` | launch-flags.md §3.1 |
| `0x00E56738`, `0x00E5673C` | данные: значения по умолчанию — `gf2` и `cuba.str miami.str queens.str` | launch-flags.md §3.1 |
| `0x00401000` | начало кода образа (`TEXT_START`); вызовы и таблицы методов должны указывать в код | crates/doctor-toolkit-live/src/addr.rs; live-link.md §16.2 |
| `0x00D5435A` | конец кода образа (`TEXT_END` в модуле) | crates/doctor-toolkit-live/src/addr.rs |
| `0x00D54FFF` | конец диапазона кода в профиле (`0x00401000–0x00D54FFF`) | live-link.md §16.2 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §2 «Точки входа в главный цикл»
- [Флаги запуска](../../analysis/launch-flags.md) §3.1, §3.4
