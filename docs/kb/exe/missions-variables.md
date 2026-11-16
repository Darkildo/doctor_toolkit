---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 48, 28, 76]
sources: ["docs/analysis/live-link.md §13.6", "docs/analysis/live-link.md §13.7", "docs/logic-graph.md §9", "docs/analysis/game-bench.md §14", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "docs/formats/save.md §4"]
tags: [exe, missions, variables]
updated: 2026-11-16
---
# Чекпоинты миссий и переменные сюжета

Адреса — виртуальные, издание PC 1.0.789.0. Текущая миссия видна по менеджеру чекпоинтов; переменные сюжета лежат массивом записей по 0x14 байт. Запись переменной модуль делает как Lua `VariableSet`: значение, затем пробуждение наблюдателей.

| Адрес | Что это | Источник |
|---|---|---|
| `0x011299C4` | данные: менеджер чекпоинтов (указатель): `+0x1C` текущий `Checkpoint`, `+0x20` список, `+0x24` число | live-link.md §13.6; crates/doctor-toolkit-live/src/addr.rs |
| `0x008FCCC0` | конструктор менеджера чекпоинтов | live-link.md §13.6 |
| `0x008FC6E0` | номер текущего чекпоинта (`+0x74`) | live-link.md §13.6; crates/doctor-toolkit-live/src/addr.rs |
| `0x008FF7E7` | место `TestCurrentCheckpoint`, где берётся менеджер чекпоинтов | live-link.md §13.6; crates/doctor-toolkit-live/src/edition.rs |
| `0x00D81474` | данные: таблица методов `Checkpoint` (модуль берёт из списка только такие объекты) | live-link.md §13.6; crates/doctor-toolkit-live/src/addr.rs |
| `0x008FC6F0` | загрузка `Checkpoint` | logic-graph.md §9 |
| `0x01223428` | данные: менеджер переменных сюжета (указатель): `+0x20` ёмкость, `+0x24` записи по 0x14 байт, `+0x28` число | live-link.md §13.7; crates/doctor-toolkit-live/src/addr.rs |
| `0x00441E60` | конструктор менеджера переменных | live-link.md §13.7 |
| `0x004423F0` | загрузчик переменных: считает `sdbm(lower(имя))` по имени из ресурса | live-link.md §13.7 |
| `0x00442130` | поиск переменной по хэшу имени (`FindVariable` Lua) | live-link.md §13.7; crates/doctor-toolkit-live/src/edition.rs |
| `0x00442550` | запись блока переменных в сохранение: пары «хэш записи, значение» у записей с битом 0 флагов (`+0xE`) | docs/formats/save.md §4 |
| `0x004425A0` | чтение блока переменных: неизвестный хэш пропускается | docs/formats/save.md §4 |
| `0x0046A641` | Lua `VariableSet`: место, где берётся менеджер переменных | crates/doctor-toolkit-live/src/addr.rs; crates/doctor-toolkit-live/src/edition.rs |
| `0x0046A666` | Lua `VariableSet`: запись значения | live-link.md §13.7 |
| `0x00442240` | `Changed(менеджер, индекс)`, `stdcall`, снимает 8 байт: будит наблюдателей переменной (`VariableWatcher`) | live-link.md §13.7; crates/doctor-toolkit-live/src/addr.rs |
| `0x00442314` | место возврата `RET 8` этой функции | crates/doctor-toolkit-live/src/edition.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §13.6, §13.7
- [Граф логики](../../logic-graph.md) §9
- [Сохранение и загрузка: адреса](saves.md)
