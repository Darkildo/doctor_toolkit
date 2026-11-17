---
type: question
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 79]
sources: ["docs/logic-graph.md §8", "docs/formats/simgroup.md §8", "data/gf2/class_behavior.toml", "docs/analysis/class-behavior.md §4"]
tags: [question, simgroup, names]
updated: 2026-11-16
---
# Имена атрибутов и классов SimGroup не восстановлены

Не названы 3594 из 3634 атрибутов и 51 из 329 классов (`CommonNPC` и др. названы, `0x82222D2E`, `0x84F8C155` — нет). У большей части параметров графа логики имён нет; названия с пометкой `guess` проверяются в игре.

## Что выяснено (этап 79)

У 45 самых используемых классов логики и мира и 5 общих пакетов названы по коду 462 атрибута (347 — смысл виден в коде, 115 — имя условное: значение хранится, использование не найдено). Из них 226 в схеме графа логики раньше не было вовсе, 72 были без имени, 9 переименованы (`TriggerBase` 0, 7, 8, 11; `TriggerFollow` 4, 5; `EnterExitTrigger` 3; `SpawnPoint` 0; `ChangeNPCSettings` 1), у 117 имя подтверждено кодом. Имена — в `data/gf2/logic_schema.toml` (их туда кладёт `research/class_behavior.py schema`).

Имена классов: реестр exe (`0x00688AF0`, 299 классов) называет и классы, которых нет в файлах игры (`VariableCombiner`, `TriggerSphere`).

## Следующий шаг

Следующие волны — классы за пределами первых 45 (список по числу объектов — `research/class_usage.py`); безымянные классы (`0x82222D2E`, `0x84F8C155`…) искать в реестре `0x00688AF0` по фабрике и размеру.

Сделано частично: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Классы SimGroup](../classes/index.md)
- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §8
