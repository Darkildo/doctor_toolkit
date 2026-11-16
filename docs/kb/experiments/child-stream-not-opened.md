---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 5, 14, 24, 45, 84]
sources: ["docs/analysis/game-bench.md §13.5", "docs/analysis/game-bench.md §13.11", "docs/analysis/game-bench.md §20.2", "docs/analysis/game-bench.md §33", "docs/analysis/new-resources.md §3.1"]
tags: [streams, toc, child-stream, enabledisablestreams]
updated: 2026-11-16
---
# Дочерний поток не открывается вместе с родителем

Три опыта с дочерним потоком (`add-stream --with-parent`, флаги `0,0,5,0`).

1. **Этап 14.** Вывод «поток `0,0,5,0` загружается вместе с родителем» сделан без трассировки
   открытий — не подтверждён.
2. **Этап 24, прогон D.** Поток `gf2/doctor/s24child` у зоны: в TOC записан как у игры (родитель
   перечисляет его в зависимостях), но файла `s24child.str` нет в `files.trace`, синей тележки на
   снимке нет; игра не падает.
3. **Этап 24, прогон K.** Поток `gf2/doctor/s24kid` у мировой зоны
   `gf2/cuba/local/xenia/interior/business_019_balcony`; запись TOC родителя — `0,1,1,0 … ->
   gf2\cuba\local, gf2\doctor\s24kid`, как у пар игры. Зона открыта, `s24kid.str` — нет. Но и
   собственный дочерний поток игры `c67_common_parentedstream_filler80_01` не открыт, хотя его
   родитель `xenia/high/filler_080_01` открыт в том же прогоне.

Вывод: запись TOC тулкита верна; дочерние потоки игра грузит не при каждой загрузке родителя, а по
условию.

**Условие найдено (этап 45):** поток грузится после действия `EnableDisableStreams` с его хэшем;
свой поток `gf2/doctor/s45kid` загружен по `live send doctor_kid_on` (`live streams` до и после).

**Место отказа в коде и поток, который грузится сам (этап 84).** Загрузка записи `0x00403C40`
просит каждый дочерний поток у `0x00404590`, а тот отказывает записи с битом `0x4`
(`LOAD_DISABLED_DEFAULT`), пока нет бита `0x8`. Бит `0x4` стоит у всех 110 дочерних потоков игры.
Дочерний поток без него (`add-stream --with-parent --auto-load`, флаги `0,0,1,0`) игра открывает
при каждой загрузке родителя и выгружает вместе с ним — [опыт этапа 84](world-grid-new-zone.md).

Что из этого следует: для своих ресурсов надёжны поток по запросу через `StreamSet`, вставка в
существующий поток или `EnableDisableStreams` (так сделан [мод «Новый Нью-Йорк»](new-new-york-mod.md)).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §13.5, §13.11, §20.2
- [Новые ресурсы и потоки](../../analysis/new-resources.md) §3.1
- [Формат TOC](../../formats/toc.md)
