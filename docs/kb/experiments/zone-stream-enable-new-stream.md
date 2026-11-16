---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 60, 84]
sources: ["docs/analysis/game-bench.md §28", "docs/analysis/queens.md §3", "docs/formats/wgi.md §6", "docs/analysis/game-bench.md §33"]
tags: [streams, zonestreamenable, enabledisablestreams, queens]
updated: 2026-11-16
---
# ZoneStreamEnable не грузит новый поток, EnableDisableStreams — грузит

Опыт прогонов 2 и 3 мода «Новый Нью-Йорк» (этап 60).

- **`ZoneStreamEnable`** в `gf2/queens/local` с именем нового потока (`CMD_SetName` — последний
  компонент пути): новый поток не грузится ни в 140, ни в 15 м от объекта. У игры так грузятся
  `venue_115` (в 20 м) и `venue_063_courtroom_game`.
- **`EnableDisableStreams`** с хэшем записи нового потока в TOC: `send_message nny_point_on` (один
  слушатель) — поток загружен через несколько кадров. Работает и для обычного (не дочернего) потока.
- **`TriggerAuto`** (`nny_boot` через 1 с) + `EnableDisableStreams` в SimGroup `gf2/queens/local`:
  поток загружен через 25 с после старта без вызовов связи и остаётся загруженным.

Гипотеза источника («видимо»): имя для `ZoneStreamEnable` ищется в мировой сетке.

**Подтверждено на этапе 84** (код `0x00890BD0`, `0x0088DBF0`): атрибут 0 объекта — имя ячейки `wgi`;
без ячейки с таким именем объект ничего не делает. С ячейкой (`world-grid add --name ИМЯ --on-enable`)
новый поток грузится, пока игрок в коробке объекта —
[опыт этапа 84](world-grid-new-zone.md).

Что из этого следует: новая суша мода — отдельный поток `gf2/queens/nny/nny_point_zonestream`
(родитель `gf2/queens/local`), который грузится сразу после входа в город; цена — около 3 МБ памяти.

Формат `wgi` и связь `ZoneStreamEnable` с ним разобраны на этапе 84:
[вопрос закрыт](../questions/wgi-format-and-zone-stream-enable.md).

Связанное: сохранение помнит бит «включено» этого потока —
[сохранения при старте `-stream`](saves-with-stream-start.md).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §28
- [Устройство Нью-Йорка](../../analysis/queens.md) §3, §7
- [Новые ресурсы и потоки](../../analysis/new-resources.md) §3.1, §3.2
