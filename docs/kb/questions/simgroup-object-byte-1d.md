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
# Смысл байта 0x1D объекта SimGroup и бита 1 флагов

У объекта SimGroup не известен смысл байта 0x1D и бита 1 флагов.

## Следующий шаг

Найти чтение этих полей в загрузчике объектов SimGroup (Ghidra); сравнить объекты с разными значениями по корпусу.

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Тип ресурса `simgroup`](../resources/simgroup.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §8
