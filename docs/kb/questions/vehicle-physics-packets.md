---
type: question
status: hypothesis
confidence: data
editions: ["1.0.789.0"]
stages: [72, 89]
sources: ["docs/analysis/tunables.md §3", "data/gf2/sim_classes.txt"]
tags: [question, vehicles, physics]
updated: 2026-11-16
---
# Физика машин: пакеты `34f699a5`, `DefaultEngine`, `DefaultTransmission`

Скорость и масса машин лежат в трёх пакетах `AICarBehavior`: `34f699a5` 0 (750, 1500, 2500, 4500 — похоже на массу), `DefaultEngine` (`50f03f03`) 4 (175–400), `DefaultTransmission` (`e9613cc5`: обороты 4000/6500 и передаточные числа). Имена двух классов — из `data/gf2/sim_classes.txt`: `sdbm("defaultengine")` = `0x50F03F03`, `sdbm("defaulttransmission")` = `0xE9613CC5`; `34f699a5` остаётся без имени. Атрибуты всех трёх не названы, в список параметров они не включены.

## Следующий шаг

Назвать класс `34f699a5` и атрибуты всех трёх классов (читатели в Ghidra), проверить массу и передачи на стенде.

Запланировано: [Этап 89: Транспорт и трафик](../research/stage-89.md).

## Связанное

- [Транспорт и трафик](../systems/vehicles.md)
- [Класс `0x34f699a5` (имя неизвестно)](../classes/class-34f699a5.md)
- [Класс `DefaultEngine`](../classes/DefaultEngine.md)
- [Класс `DefaultTransmission`](../classes/DefaultTransmission.md)
- [docs/analysis/tunables.md](../../analysis/tunables.md) §3
