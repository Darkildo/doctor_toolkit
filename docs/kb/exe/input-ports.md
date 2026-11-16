---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 45]
sources: ["docs/analysis/live-link.md §5.1", "docs/analysis/game-bench.md §20.1", "docs/analysis/launch-flags.md §3", "docs/analysis/performance.md §3", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, input, player]
updated: 2026-11-16
---
# Ввод: порт контроллера и карты действий

Адреса — виртуальные, издание PC 1.0.789.0. Игрок читает ввод с порта, который получает в конструкторе от менеджера контроллера. При старте с `-stream` игрок создаётся раньше, чем у менеджера появляется порт, и клавиатура не работает; модуль чинит это, перекладывая карты действий на нужный порт.

| Адрес | Что это | Источник |
|---|---|---|
| `0x01129814` | данные: менеджер контроллера (указатель); `+0x14` — порт (байт) того, кто играет: `0x0F` клавиатура и мышь, меньше `0x10` устройство, `0x12`/`0x13` — нет | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0068CBC0`, `0x0068C7A0`, `0x0068C7E0` | функции менеджера контроллера, по которым выведено поле порта | crates/doctor-toolkit-live/src/addr.rs |
| `0x007A2A5A` | конструктор `Player`: порт менеджера → `Player+0x2498` и `0x00444D20(порт, 1)` | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00444D20` | менеджер ввода: пометить порт занятым (байт `+0x14 + порт * 0x18`) | live-link.md §5.1; crates/doctor-toolkit-live/src/edition.rs |
| `0x01223430` | данные: менеджер ввода (указатель); записи портов по `0x18` байт с `+0x14` | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0067D1ED` | обновление `GameMainLoop`: когда уровень загружен и порта у менеджера нет — даёт порт последнего устройства | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x012233B4` | данные: объект, из которого берётся порт последнего устройства (`+0x53D`) | live-link.md §5.1 |
| `0x0079E920` | действие игрока: бит 1 `Player+0x249C` и `0x00465700(порт игрока, действие)` | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00465700` | запрос действия на порту | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00465660` | ищет действие только в картах порта | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x01223464` | данные: карты действий портов (указатель): на порт блок `0x168` байт, `+0x10` число карт стека, `+0x14` стек | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x01205400` | данные: карты действий (упомянуты в проверке издания как `maps 0x01205400`) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00464930` | имя карты действий — в `+9` | crates/doctor-toolkit-live/src/addr.rs; crates/doctor-toolkit-live/src/edition.rs |
| `0x00464960` | `bool Push(this, u8 порт, const char* имя, bool)`, `thiscall`, снимает 12 байт: кладёт карту на порт | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00464A10` | `Pop(this, u8 порт)`, `thiscall`, снимает 4 байта: снимает верхнюю карту порта | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0079B5EE` | место, где игрок кладёт свою карту (`basic_aiming_player`) на свой порт | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0079B5BD` | начало этого места в обработчике сообщений игрока (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x007A7485` | деструктор `Player`: снимает карту с порта | live-link.md §5.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0040F9C0` | флаг `-inputwithoutfocus`: пишет байт `0x0120524A` | launch-flags.md §3 |
| `0x0120524A` | данные: байт «ввод без фокуса окна» | launch-flags.md §3 |
| `0x0040FF6F` | место оконной процедуры, читающее `0x0120524A` | launch-flags.md §3 |
| `0x00B9513F`, `0x00B9519F`, `0x00B95223`, `0x0040FE3A` | опрос устройств ввода на главном потоке (метод `+0x64` — `Poll`) | performance.md §3; live-link.md §16.6 |
| `0x0069D71F`, `0x0069D75B` | `PeekMessageW` на главном потоке | performance.md §3; live-link.md §16.6 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §5.1 «Управление после старта сразу в уровень»
- [Стенд](../../analysis/game-bench.md) §20.1
