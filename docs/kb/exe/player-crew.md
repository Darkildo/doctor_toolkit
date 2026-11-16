---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 48]
sources: ["docs/analysis/live-link.md §13.5", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "crates/doctor-toolkit-live/src/state.rs"]
tags: [exe, family, crew]
updated: 2026-11-16
---
# Команда игрока

Адреса — виртуальные, издание PC 1.0.789.0. Команда игрока — семь мест по 0x24 байта; модуль её только читает (запись — добавление NPC — не делается).

| Адрес | Что это | Источник |
|---|---|---|
| `0x0112989C` | данные: команда игрока (указатель): семь мест по 0x24 байта с `+0x18` | live-link.md §13.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0090A370` | обработчик класса `AddPlayerFamilyMember` | live-link.md §13.5 |
| `0x0090A3DA` | место `AddPlayerFamilyMember`, где берётся команда `*0x0112989C` | crates/doctor-toolkit-live/src/edition.rs |
| `0x0090B0A0` | обход семи мест команды | live-link.md §13.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0090B690` | флаги места: бит 0 — открыто, бит 1 — занято | live-link.md §13.5 |
| `0x0090A5B0` | член команды = ручка `+8` − 0x48 | live-link.md §13.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0090A5C5` | место чтения специальностей места (`+0x10`) | live-link.md §13.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0090A5C0` | начало этого места (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x009630E0` | специальности: 0x04 подрывник … 0x80 громила | live-link.md §13.5 |
| `0x0079179B` | место чтения ранга члена (`+0xD0`) | live-link.md §13.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00791750` | функция ранга члена (по state.rs) | crates/doctor-toolkit-live/src/state.rs |
| `0x00791670` | запись ранга члена | live-link.md §13.5 |
| `0x0090B4E0` | добавление NPC в команду (модуль не делает) | live-link.md §13.5 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §13.5 «Семья: команда игрока»
