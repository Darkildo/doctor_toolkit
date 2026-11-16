---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 79]
sources: ["docs/formats/simgroup.md §8"]
tags: [question, simgroup]
updated: 2026-11-16
---
# Что задают атрибуты 2 и 3 `CSystemCommands`

Атрибуты 2 и 3 `CSystemCommands` — int, почти у всех объектов равны 1. Что они задают, не известно.

## Следующий шаг

Разобрать читатель `CSystemCommands` в Ghidra; найти объекты со значением не 1 и сравнить их поведение.

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Класс `CSystemCommands`](../classes/CSystemCommands.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §8
