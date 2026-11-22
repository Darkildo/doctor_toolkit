---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 48, 114]
sources: ["docs/analysis/live-link.md §13.4", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "docs/analysis/dev-menu-cheat-table.md §0.1"]
tags: [exe, money, family]
updated: 2026-11-20
---
# Семьи и деньги

Адреса — виртуальные, издание PC 1.0.789.0. Семьи лежат списком; у семьи есть деньги и книги дохода по категориям. Модуль начисляет деньги штатной `AddMoney` с категорией 5, как награды игры.

| Адрес | Что это | Источник |
|---|---|---|
| `0x01129904` | данные: семьи (указатель): `+0x24` список, `+0x28` число; хэш имени семьи `+0x54` | live-link.md §13.4; crates/doctor-toolkit-live/src/addr.rs |
| `0x008C74D0` | поиск семьи по хэшу имени | live-link.md §13.4; crates/doctor-toolkit-live/src/addr.rs |
| `0x01129908` | данные: объект с копией денег для счётчика на экране (`+0x14`); он же — заслуги и лицензии людей (`+8`, `+0xC`, см. [команду](player-crew.md)) | live-link.md §13.4; dev-menu-cheat-table.md §0.1 |
| `0x0093B137` | место, где счётчик на экране берёт копию денег | live-link.md §13.4 |
| `0x0093B108` | начало места счётчика денег: семьи и семья игрока по хэшу `0x0637B907` (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x0093B10E` | место с хэшем `corleone` (семья игрока) | crates/doctor-toolkit-live/src/addr.rs |
| `0x0093B126` | место чтения книг дохода семьи | crates/doctor-toolkit-live/src/addr.rs |
| `0x0093B0E0` | обновление счётчика денег на экране | live-link.md §13.4 |
| `0x008C09A0` | `f32 AddMoney(this, сумма, категория)`, `thiscall`, `RET 8`, результат в `ST0` | live-link.md §13.4; crates/doctor-toolkit-live/src/addr.rs |
| `0x008C09E5` | место `AddMoney` с хэшем семьи игрока | crates/doctor-toolkit-live/src/addr.rs |
| `0x008C09F2` | место `AddMoney`, где деньги ограничиваются снизу нулём | crates/doctor-toolkit-live/src/addr.rs |
| `0x00892910` | книги: доход по категориям (`+0xA4`, число `+0xD0`) | live-link.md §13.4 |
| `0x008BCAF0` | для `corleone`: копия для счётчика и его обновление | live-link.md §13.4; crates/doctor-toolkit-live/src/addr.rs |
| `0x007DD518` | место вызова `AddMoney` из наград игры | live-link.md §13.4; crates/doctor-toolkit-live/src/addr.rs |
| `0x007E787A` | место вызова `AddMoney` из наград игры с категорией 5 | live-link.md §13.4; crates/doctor-toolkit-live/src/addr.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §13.4 «Деньги»
- Люди семей, вендетта, заведения, услуги — [семьи: люди, вендетта, заведения](family-members.md)
