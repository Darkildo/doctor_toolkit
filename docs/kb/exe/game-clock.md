---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58, 59, 78]
sources: ["docs/analysis/message-bus.md §3.2", "docs/analysis/performance.md §7", "docs/analysis/live-link.md §3", "docs/analysis/live-link.md §16.1", "docs/analysis/live-link.md §16.5", "crates/doctor-toolkit-live/src/addr.rs"]
tags: [exe, clock, timing]
updated: 2026-11-16
---
# Часы игры: такт, шаг кадра, время симуляции

Адреса — виртуальные, издание PC 1.0.789.0. Часы игры считают такты симуляции и шаг кадра. Шаг в игре переменный (байт фиксированного шага всегда 0), темп игры от частоты кадров не зависит до нижнего предела 12 к/с.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00409F80` | часы игры: обработчик сообщения такта | performance.md §7 |
| `0x01223390` | данные: объект часов игры (указатель) | performance.md §7 |
| `0x0110AE4C` | данные: частота такта (`f32`, по умолчанию 59,94); ставит `0x00412BD0` | performance.md §7; crates/doctor-toolkit-live/src/addr.rs |
| `0x00412BD0` | ставит частоту такта по частоте обновления экрана из настроек (`0x00986D10`) | performance.md §7; renderer.md §2 |
| `0x0040A20C` | место деления на частоту такта `0x0110AE4C` | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x01206A30` | данные: длина такта, мс = 1000 / частота | performance.md §7 |
| `0x0110AE75` | данные: байт «фиксированный шаг» (1 — фиксированный, 0 — переменный); в игре 0 | performance.md §7; crates/doctor-toolkit-live/src/addr.rs |
| `0x0067C130` | запись байта фиксированного шага `0x0110AE75` | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00409D87` | чтение байта фиксированного шага | crates/doctor-toolkit-live/src/addr.rs |
| `0x0067CF36` | вход `InitOnceMainLoop`: всегда пишет 0 в `0x0110AE75` | performance.md §7 |
| `0x0110AE7C` | данные: предел тактов за кадр, 5 | performance.md §7 |
| `0x01206800`, `0x012067E8` | данные: шаг кадра, с (время кадра по `timeGetTime`, от 1 мс до 5 тактов); читают больше сотни мест кода | performance.md §7 |
| `0x01205210` | данные: время игры, мс | performance.md §7 |
| `0x01205224` | данные: время симуляции, мс (`u32`); его читает Lua `GetSimTime` | live-link.md §3; performance.md §7; crates/doctor-toolkit-live/src/addr.rs |
| `0x0046A850` | Lua `GetSimTime`: читает `0x01205224` | live-link.md §3; crates/doctor-toolkit-live/src/edition.rs |
| `0x0040D450` | ограничитель фиксированного шага (ждать до `тактов / частота`) — мёртвый код, никто не вызывает | performance.md §7; renderer.md §4 |
| `0x00440590` | очередь задержки сообщений: срок записи — `0x01205224` + секунды × 1000 (прежняя запись называла `0x004404F0` и `0x010F6F2C` — неверно) | message-bus.md §3.2 |
| `0x01129A8C` | данные: счётчик кадров игры (`u32`); `0x0068BAF0` прибавляет 1 в начале кадра | live-link.md §16.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x011260B0` | данные: константа 59,94 (тики анимации в секунду) | characters.md §6.4 |

## Подробнее

- [Производительность](../../analysis/performance.md) §7 «Темп игры и частота кадров»
- [Живая связь](../../analysis/live-link.md) §16.1, §16.5
