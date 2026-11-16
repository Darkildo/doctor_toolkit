---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 79]
sources: ["docs/logic-graph.md §8", "docs/formats/simgroup.md §8"]
tags: [question, simgroup, names]
updated: 2026-11-16
---
# Имена атрибутов и классов SimGroup не восстановлены

Не названы 3594 из 3634 атрибутов и 51 из 329 классов (`CommonNPC` и др. названы, `0x82222D2E`, `0x84F8C155` — нет). У большей части параметров графа логики имён нет; названия с пометкой `guess` проверяются в игре.

## Следующий шаг

Для самых используемых классов разобрать читатели и обработчики (волна 1), пополнить `data/gf2/logic_schema.toml` и `sim_classes.txt`, пересобрать заметки классов (`kb-gen`).

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Классы SimGroup](../classes/index.md)
- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §8
