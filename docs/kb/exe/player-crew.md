---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 48, 82]
sources: ["docs/analysis/crew.md", "docs/analysis/live-link.md §13.5", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "crates/doctor-toolkit-live/src/state.rs"]
tags: [exe, family, crew]
updated: 2026-11-16
---
# Команда игрока

Адреса — виртуальные, издание PC 1.0.789.0. Семья игрока — семь мест по 0x24 байта; модуль её только читает. Запись в место делает сама игра по данным: объект `AddPlayerFamilyMember` или разговор с рекрутом (этап 82).

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
| `0x0090B4E0` | запись человека в место семьи: `(ранг, SimNpc, специальности, место, GUID оружия)`; ищет открытое свободное место ранга | crew.md §4 |
| `0x0090AA60` | заполнение места: флаг «занято», ручка, специальности, оружие; `SimNpc` получает ранг места и семью `corleone` | crew.md §4 |
| `0x0090A1D0` | читатель класса `AddPlayerFamilyMember` | crew.md §4; data/gf2/class_behavior.toml |
| `0x0090B220` | найм рекрута разговором: первое открытое свободное место из четырёх солдатских | crew.md §4 |
| `0x0090B750` | запись `SimNpc` при создании (для семьи `corleone` закрыта битом 6 флагов) | crew.md §4 |
| `0x0090BA70` | обработчик сообщений `iMsgTreeType…`: уровень древа `+0x14`, 1…8 | crew.md §3 |
| `0x0090B480` | открывает места по уровню древа | crew.md §3 |
| `0x0090B8E0` | уровень древа из сохранения | crew.md §3 |
| `0x0090B330` | повышение: перенос человека из места в место, запись ранга | crew.md §6 |
| `0x0090B820` | человек выбыл: приговорённый убирается из места и семьи, остальные — только из команды | crew.md §7 |
| `0x0090B5D0` | освобождение места (`0x0090A9F0` чистит его) | crew.md §7 |
| `0x0090A8C0` | проверка «приговорён»: бит 29 флагов `SimNpc` | crew.md §7 |
| `0x00790000` | специальности шаблона: `NPCCrewComponent.0 & 0xFC` | crew.md §2 |
| `0x00790150` | начальная оружейная лицензия по флагам `SimNpc` (бит 3 — 2, бит 4 — 3) | crew.md §8 |
| `0x008BCF90` | запись заслуг человека: GUID и лицензия | crew.md §8 |
| `0x008BCE30` | прибавить заслугу; лицензия по сумме: 10, 25, 70 | crew.md §8 |
| `0x009263B0` | экран сведений о рекруте: специальности, имя, биография (`имя + "_background"`), лицензия | crew.md §2, §9 |
| `0x00906860` | обработчик класса `UnlockCheck`: флаг дополнения `*0x01129908 +0x18` | crew.md §10 |
| `0x009BED80` | сборка персонажа `mobface` для портрета | crew.md §9 |

## Подробнее

- [Команда: люди чести семьи игрока](../../analysis/crew.md)
- [Живая связь](../../analysis/live-link.md) §13.5 «Семья: команда игрока»
