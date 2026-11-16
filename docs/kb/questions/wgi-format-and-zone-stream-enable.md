---
type: question
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 84]
sources: ["docs/formats/wgi.md", "docs/analysis/new-resources.md §3.5", "docs/analysis/game-bench.md §33", "research/wgi_survey.py"]
tags: [question, zones, stream, wgi]
updated: 2026-11-16
---
# Формат `wgi` и связь `ZoneStreamEnable` с ним

Решено на этапе 84.

- `wgi` — мировая сетка раздела `WorldGraphPartition`: таблица потоков уровня, ячейки (поток зоны с центром, коробкой, видом, маской LOD и именем) и пул ссылок. Разбор и запись — `resource::wgi`, все 28 ресурсов побайтно.
- `ZoneStreamEnable` ищет ячейку по SDBM своего атрибута 0 (поле 0x60 ячейки) и держит её включённой, пока игрок внутри коробки объекта. Имени нового потока в сетке не было — поэтому объект ничего не делал.
- Новая зона добавляется в сетку командой `world-grid add` (MCP `add_zone_cell`); в игре (Гавана, Нью-Йорк) поток приходит и уходит по положению игрока.

## Следующий шаг

Вопрос закрыт. Что осталось: [флаги ячеек и коробка потока](wgi-cell-flags-and-stream-box.md); ячейки в сетках средней и низкой детализации в игре не проверялись.

## Связанное

- [Зоны мира и их подгрузка](../engine/zones.md)
- [Мировая сетка: функции и данные](../exe/world-grid.md)
- [Тип ресурса `wgi`](../resources/wgi.md)
- [Новый поток в мировой сетке, дочерний поток и `ZoneStreamEnable`](../experiments/world-grid-new-zone.md)
- [ZoneStreamEnable не грузит новый поток, EnableDisableStreams — грузит](../experiments/zone-stream-enable-new-stream.md)
- [Формат `wgi`](../../formats/wgi.md)
