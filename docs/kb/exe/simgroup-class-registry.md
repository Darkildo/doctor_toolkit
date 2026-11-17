---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 16, 32, 80]
sources: ["docs/analysis/live-link.md §4", "docs/analysis/live-link.md §12.2", "docs/analysis/live-link.md §13.1", "docs/analysis/live-link.md §13.2", "docs/analysis/live-link.md §13.3", "docs/analysis/live-link.md §13.5", "docs/analysis/live-link.md §15.1", "docs/logic-graph.md §1", "docs/analysis/launch-flags.md §4", "crates/doctor-toolkit-live/src/addr.rs", "docs/analysis/asi-platform.md §2"]
tags: [exe, simgroup, classes]
updated: 2026-11-16
---
# Реестр классов SimGroup и читатели атрибутов

Адреса — виртуальные, издание PC 1.0.789.0. Классы SimGroup регистрируются одной функцией из одного места; реестр — отсортированный массив записей `{хэш, фабрика, имя, размер}`. Читатели атрибутов найдены как вызывающие общую функцию чтения.

| Адрес | Что это | Источник |
|---|---|---|
| `0x004833A0` | регистрация класса SimGroup: хэш, имя, фабрика, размер | logic-graph.md §1; live-link.md §4 |
| `0x00483410` | создание объекта по записи: поиск класса в реестре (`0x004838C0`), `фабрика(запись, доп)`, затем читатель — метод `+4` объекта | asi-platform.md §2 |
| `0x004837E0` | вставка записи в массив реестра (сортировка по хэшу, массив растёт); хэш, который уже есть, не регистрируется второй раз (`0x00483720` возвращает 0) | asi-platform.md §2 |
| `0x0068AB55` | место вызова `0x00688AF0` в старте приложения `0x0068AB50` | asi-platform.md §2 |
| `0x00688AF0` | функция, из которой идут все вызовы `0x004833A0` (реестр классов) | logic-graph.md §1; live-link.md §12.2 |
| `0x00483720` | работа с реестром классов (упомянута вместе с `0x004833A0`) | crates/doctor-toolkit-live/src/addr.rs |
| `0x012239BC` | данные: реестр классов — указатель на отсортированный массив, `+4` число; запись `{хэш, фабрика, имя, размер}` | live-link.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0043AFF0` | общая функция чтения атрибутов; читатели классов найдены как её вызывающие (`switch` по индексу) | logic-graph.md §1 |
| `0x0068A53E`, `0x0068A557`, `0x0068A573` | места рядом с регистрацией имён `GameplayTrackingTweaker`, `PlayerTargetingTrackingTweaker`, `ViolenceTrackingTweaker` | launch-flags.md §4 |
| `0x00684AE0` | фабрика класса `TimeOfDayManager` | live-link.md §13.1 |
| `0x00684B10` | фабрика класса `SetTimeOfDay` | live-link.md §13.1 |
| `0x00684E60` | фабрика класса `WeatherManager` | live-link.md §13.2 |
| `0x00684960`, `0x00684990` | фабрики классов `LoadStream` и `UnloadStream` | live-link.md §13.3 |
| `0x006844C0` | фабрика класса `AddPlayerFamilyMember` | live-link.md §13.5 |
| `0x008CB230` | фабрика части коллизии разрушаемых объектов | live-link.md §15.1 |

## Подробнее

- [ASI-платформа](../../analysis/asi-platform.md) §2 — как создаётся объект класса и как класс регистрируется из DLL; адреса: [платформа нативных модов](native-mods.md)
- [Граф логики](../../logic-graph.md) таблица функций, раздел о читателях
- [Живая связь](../../analysis/live-link.md) §4
- [Формат SimGroup](../../formats/simgroup.md)
