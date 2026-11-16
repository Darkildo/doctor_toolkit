---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 46, 58, 59]
sources: ["docs/analysis/live-link.md вводная часть", "docs/analysis/live-link.md §6", "docs/analysis/live-link.md §14.2", "docs/analysis/live-link.md §16.5", "docs/analysis/live-link.md §9", "crates/doctor-toolkit-live/src/edition.rs", "crates/doctor-toolkit-live/src/render.rs", "crates/doctor-toolkit-render-asi/src/lib.rs"]
tags: [exe, edition, hooks]
updated: 2026-11-16
---
# Проверка издания: что сверяется

Адреса — виртуальные, издание PC 1.0.789.0. Модули трогают игру только после проверки: заголовок PE (`MZ`, `PE`, `TimeDateStamp = 0x4AC976E5`, `SizeOfImage = 0x00E32000`) и байты каждой функции и команды, которую они вызывают, правят или из которой взят адрес. Образ без перемещаемых адресов всегда грузится с одной базы. Места сверки перечислены в тематических заметках с пометкой «сверяется при проверке издания».

| Адрес | Что это | Источник |
|---|---|---|
| `0x00400000` | база образа `godfather2.exe` (перемещаемых адресов нет) | live-link.md вводная часть; crates/doctor-toolkit-live/src/edition.rs |
| `0x01232000` | конец образа (база + `SizeOfImage`); модуль рендера принимает адреса только до него | crates/doctor-toolkit-render-asi/src/lib.rs |
| `0x00608930`, `0x0069DE10`, `0x0069E840`, `0x00682860`, `0x009BCB30` | функции, которые перехватывает мод игрока GF2ASI (вместе с `0x00403A50`, `0x00608250`, `0x006817C0`); с модулем уживается — места разные | live-link.md §9 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §6 «Проверка издания», §14.2, §16.5
- [Производительность](../../analysis/performance.md) §10 (таблица патчей модуля рендера)
