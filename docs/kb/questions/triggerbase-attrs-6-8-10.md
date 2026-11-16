---
type: question
status: hypothesis
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 27, 79]
sources: ["docs/formats/mission-logic.md §6"]
tags: [question, simgroup, logic, triggers]
updated: 2026-11-16
---
# Смысл атрибутов 6 и 8–10 `TriggerBase`

Названия атрибутов 6 и 8–10 `TriggerBase`, данные на этапе 27, помечены `guess`: по данным и коду читателя смысл не доказан.

## Следующий шаг

Разобрать читатель и обработчики `TriggerBase` в Ghidra; изменить атрибут у видимого триггера и проверить поведение на стенде.

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Класс `TriggerBase`](../classes/TriggerBase.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §4, §6
