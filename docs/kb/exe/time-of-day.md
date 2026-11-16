---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 48, 58, 59]
sources: ["docs/analysis/live-link.md §3", "docs/analysis/live-link.md §13.1", "docs/analysis/performance.md §7", "docs/analysis/performance.md §10.1", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "crates/doctor-toolkit-live/src/render.rs"]
tags: [exe, time-of-day, world]
updated: 2026-11-16
---
# Время суток: TimeOfDayManager

Адреса — виртуальные, издание PC 1.0.789.0. `TimeOfDayManager` — объект SimGroup с часами уровня; смена времени идёт штатным сеттером, который рассылает изменение по шине. Такт привязан к кадрам и теряет остаток счётчика — модуль рендера это исправляет.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0112A53C` | данные: объект `TimeOfDayManager` (указатель); год и день `+0x5C`, `+0x60`, часы, минуты, секунды `+0x64…+0x6C`, скорость `+0x70` | live-link.md §3; live-link.md §13.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00826EB0` | конструктор `TimeOfDayManager` (размер 0xB4) | live-link.md §13.1 |
| `0x00826510` | чтение года, дня, часов, минут, секунд | live-link.md §13.1; crates/doctor-toolkit-live/src/edition.rs |
| `0x00826B70` | такт менеджера на `iMsgSystemTick`: двигает часы на скорость | live-link.md §13.1; performance.md §7 |
| `0x00826370` | перевод шага кадра в единицы счётчика (1/1200 игровой секунды) | performance.md §7; crates/doctor-toolkit-live/src/render.rs |
| `0x00826410` | выдаёт целые секунды времени суток | performance.md §8; crates/doctor-toolkit-live/src/render.rs |
| `0x00826BCD` | начало места такта со счётчиком 1/1200 с (сверяется при проверке издания модуля рендера) | crates/doctor-toolkit-live/src/render.rs |
| `0x00826BF5` | `mov dword [edi], 0` — обнуление счётчика; патч модуля рендера: `call` своего кода, остаток сохраняется | performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x0112DB64` | данные: сообщение начала нового часа | live-link.md §13.1 |
| `0x0112DB6C` | данные: сообщение полуночи | live-link.md §13.1 |
| `0x0112DB7C` | данные: сообщение каждого такта | live-link.md §13.1 |
| `0x0112DB74` | данные: сообщение «время изменено» (шлёт `SetTime`) | live-link.md §13.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x008269F0` | `TimeOfDayManager::SetTime(this, год, день, час, минута, секунда)`, `thiscall`, `RET 0x14` | live-link.md §13.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x008268F0` | зажимает значения времени (из `SetTime`) | live-link.md §13.1 |
| `0x0112A55C` | данные: объект, состояние которого сбрасывает `SetTime` | live-link.md §13.1 |
| `0x007EC440` | сброс состояния объекта `*0x0112A55C` (из `SetTime`) | live-link.md §13.1 |
| `0x00825B90` | обработчик класса `SetTimeOfDay` | live-link.md §13.1 |
| `0x00825BA6` | место `SetTimeOfDay`, где берётся `TimeOfDayManager` | crates/doctor-toolkit-live/src/edition.rs |
| `0x00825D41` | место `SetTimeOfDay` с вызовом сеттера времени (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00825D43` | место вызова `SetTime` в `SetTimeOfDay` | live-link.md §13.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00825D4E` | место: при флаге 2 скорость `+0x70 = +0x74` | live-link.md §13.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00825D6A` | место: скорость из атрибута, если он не меньше 0 | live-link.md §13.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0082DF70` | линейная интерполяция между ключами времени суток, в том числе через полночь | renderer.md §10.2 |
| `0x0082CDA0` | конструктор `SetFogMessage`: регистрирует ключ в последовательности `TODSequencer` | live-link.md §13.2 |
| `0x009CBEE0` | значения тумана по умолчанию: от 500 до 1000 м, без цвета | crates/doctor-toolkit/src/time_of_day.rs |
| `0x009D3650` | значения света по умолчанию | crates/doctor-toolkit/src/time_of_day.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §13.1 «Время суток»
- [Производительность](../../analysis/performance.md) §7, §10.1
