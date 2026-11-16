---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [84]
sources: ["docs/formats/wgi.md", "docs/analysis/new-resources.md §3.5", "docs/analysis/game-bench.md §33", "docs/formats/simgroup.md §1.1", "research/wgi_survey.py"]
tags: [research, zones, wgi]
updated: 2026-11-16
---
# Этап 84: Загрузка зон

- Состояние: сделан (2026-11-16).
- Зависит от: [этап 72](stage-72.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Формат [`wgi`](../resources/wgi.md) разобран по коду и корпусу: разбор и запись всех 28 ресурсов побайтно, добавление и удаление ячеек — [формат](../../formats/wgi.md).
- Как игра выбирает зоны: приоритет по расстоянию от игрока, радиусы разделов, бюджеты по `data_size`, предпосылки — [зоны](../engine/zones.md), [адреса](../exe/world-grid.md).
- `ZoneStreamEnable` включает ячейку сетки по имени — [вопрос закрыт](../questions/wgi-format-and-zone-stream-enable.md).
- Дочерний поток не приходит с родителем из-за бита «выключен» в записи TOC; без бита приходит — [опыт](../experiments/child-stream-not-opened.md).
- Хэш группы SimGroup — хэш потока в TOC — [вопрос закрыт](../questions/simgroup-group-hash.md).
- Новые зоны грузятся как родные: `world-grid add`, `add-stream --auto-load`, MCP `add_zone_cell`; проверено в игре — [опыт](../experiments/world-grid-new-zone.md).

## Что осталось

- [Флаги ячеек, связанная ячейка, коробка потока](../questions/wgi-cell-flags-and-stream-box.md).
- Не разбирались: [u16 в заголовке потока](../questions/stream-header-u16-3.md), [GUID кадров роликов и банков диалогов](../questions/stream-guid-cutscenes-dialogue-banks.md), [атрибуты `ZoneWorld`](../questions/zoneworld-and-traffic-attributes.md), [кадр прибытия зоны](../questions/main-thread-zone-arrival.md).
- В игре не проверены сетки средней и низкой детализации, Майами, сохранения с новыми ячейками (Гавана и Нью-Йорк проверены).

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
