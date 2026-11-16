---
type: question
status: hypothesis
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 27, 79]
sources: ["docs/formats/mission-logic.md §6", "docs/logic-graph.md §9"]
tags: [question, simgroup, logic]
updated: 2026-11-16
---
# Смысл атрибутов 1 и 2 `MultiManager`

По коду: атрибут 1 — число срабатываний (−1 у 9 015 объектов), атрибут 2 — флаги. Названия этапа 27 помечены `guess`.

## Следующий шаг

Разобрать обработчик `MultiManager` (читатель `0x47A320`) до использования атрибутов 1 и 2; проверить на стенде объектом с числом срабатываний 1.

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Класс `MultiManager`](../classes/MultiManager.md)
- [Сюжет и миссии](../systems/story.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §4, §6
