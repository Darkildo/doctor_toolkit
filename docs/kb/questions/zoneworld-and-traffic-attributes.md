---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 84, 89]
sources: ["docs/analysis/performance.md §9", "docs/analysis/performance.md §6"]
tags: [question, zones, traffic]
updated: 2026-11-16
---
# Смысл атрибутов `ZoneWorld`, `ZoneRenderLOD.1`, `TrafficManager.*`

Числа есть, читатели не разобраны; опыт с изменением `TrafficManager.*` разницы не дал. Параметры плотности людей не найдены.

Этап 84 показал, что радиусы и бюджеты загрузки зон задаёт не `ZoneWorld`, а мировая сетка: радиус раздела по маске `WorldGraphMetrics` (200 / 500 / 2 000 / 5 000 м), бюджет — его атрибут 4 ([зоны](../engine/zones.md)). Читатели `ZoneWorld`, `ZoneRenderLOD` и `TrafficManager` на этапе 84 не разбирались.

## Следующий шаг

Разобрать читатели `ZoneWorld`, `ZoneRenderLOD`, `TrafficManager` в Ghidra.

Запланировано: [Этап 84: Загрузка зон](../research/stage-84.md), [Этап 89: Транспорт и трафик](../research/stage-89.md).

## Связанное

- [Зоны мира и их подгрузка](../engine/zones.md)
- [Транспорт и трафик](../systems/vehicles.md)
- [Класс `TrafficManager`](../classes/TrafficManager.md)
- [Плотность машин и пешеходов: атрибуты TrafficManager не дали разницы](../experiments/traffic-density-no-effect.md)
- [docs/analysis/performance.md](../../analysis/performance.md) §6, §9
