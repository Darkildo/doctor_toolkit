---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 87]
sources: ["docs/analysis/tunables.md §3"]
tags: [question, police]
updated: 2026-11-16
---
# Параметры полиции: `CrimeDefinitions`, `RoadblockManager`, `ChaseConfig`, `ChaseManager`, `WitnessCrime`

`CrimeDefinitions` — по объекту на город, 205 атрибутов, 22 повторяющихся блока по 8 (вероятно, виды преступлений); `CrimeManager` пуст; `RoadblockManager` (50, 75, 155, 225, 105, 10 — похоже на пороги «жара», не доказано); `ChaseConfig` 2/3/7/8 растут с уровнем (возможно, число машин и полицейских); `ChaseManager` — 146 атрибутов; `WitnessCrime` 3 (150).

## Следующий шаг

Разобрать читатели этих классов в Ghidra и назвать атрибуты; проверить пороги на стенде.

Запланировано: [Этап 87: Полиция и закон](../research/stage-87.md).

## Связанное

- [Полиция и закон](../systems/police.md)
- [Класс `CrimeDefinitions`](../classes/CrimeDefinitions.md)
- [Класс `RoadblockManager`](../classes/RoadblockManager.md)
- [Класс `ChaseConfig`](../classes/ChaseConfig.md)
- [Класс `ChaseManager`](../classes/ChaseManager.md)
- [Класс `WitnessCrime`](../classes/WitnessCrime.md)
- [docs/analysis/tunables.md](../../analysis/tunables.md) §3
