---
type: system
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 73, 11, 37, 48, 60, 63, 84]
sources: ["docs/levels.md §1, §3, §11.1", "docs/analysis/queens.md §1–§5, §7, §8", "docs/analysis/live-link.md §13.1–13.2", "data/gf2/enums.toml", "docs/formats/wgi.md §4"]
tags: [world, levels, zones, time-of-day, weather, queens]
updated: 2026-11-16
---
# Мир: города, зоны, время суток

Мир — три города (`gf2/miami`, `gf2/cuba`, `gf2/queens`; в данных они называются `florida`, `cuba`, `newyork` — атрибут 0 объекта `City`, перечисление [`city`](../enums/city.md)) и общий поток `gf2/globals_global`; каждый город разбит на зоны-потоки с SimGroup. Загрузка зон — см. [зоны](../engine/zones.md) и [загрузка потоков](../engine/stream-loading.md).

## Что известно

- Зона — поток с SimGroup (2006 в игре, по одной на поток): `gf2/miami` — 797 зон и 49 092 объекта, `gf2/cuba` — 483 и 22 148, `gf2/queens` — 532 и 21 335, `globals_global` — 10 188 объектов; `mp_*` и `wb_multiplayer` — 150 зон мультиплеера.
- Пути: `gf2/<город>/local/<район>/high/terrainNN` (полная детализация), `…/medium/zoneN`, `…/low/zone1`, `…/interior/…`; задания — `gf2/<город>/global/objectives/…` (см. [сюжет](story.md)).
- Положение объекта — `CSystemCommands.CMD_LoadMatrix` (Y вверх); террейн (`ZoneRenderLOD`) стоит в начале координат. Земля улиц — в коллизии зон `medium`.
- Нью-Йорк: 537 потоков; корень `gf2/queens` — [City](../classes/City.md), 66 `StreamSet`, 75 `Checkpoint`; пять районов `genna`, `hank`, `heidi`, `ham`, `homer`. `City` задаёт рамку карты X −930…450, Z −750…590; уровень улиц — 6,4 м.
- Граница мира: 11 объёмов `TriggerHurt` «boundary_fade_out» под водой до высоты ≈0,9 м — игрок в воде гаснет и возвращается.
- Мировая сетка: 4 `WorldGraphPartition` с ресурсами `wgi` и мерами `WorldGraphMetrics` (маска LOD и бюджет: высокая 25 МБ, средняя 20, низкая 8, карта «Взгляда дона» 8); зоны грузятся по расстоянию от игрока до центра ячейки (до 180 / 450 / 1 800 м), пока хватает бюджета. `ZoneStreamEnable` включает ячейку по имени, пока игрок внутри его коробки; `EnableDisableStreams` ставит потоку бит принудительной загрузки. Свой поток добавляется в сетку (`world-grid add`) — см. [зоны](../engine/zones.md).
- Карта: `Map3dEntity` в `gf2/queens/local` — она же мини-карта; метки — `UIHudMiniMapMarker` (атрибут 9 — показать, 11 — скрыть).
- Время суток — `TimeOfDayManager` (`*0x0112A53C`, один объект в `globals_global`: скорость 60, год 1950, час 16): часы `+0x64`/`+0x68`/`+0x6C`, скорость `+0x70` (игровых секунд за реальную), `SetTime` — `0x008269F0`. Небо, туман, свет — ключи `TODMessage` (`SetSkyMessage` 234, `SetFogMessage` 170).
- Погода — класс `WeatherManager` (дождь, `*0x0112AFF4`); в одиночной игре дождя нет: ни одна из 2 006 SimGroup его не создаёт.
- Мод «Новый Нью-Йорк» — пример новой суши: отдельный поток `gf2/queens/nny/nny_point_zonestream` (родитель `gf2/queens/local`), в SimGroup `gf2/queens/local` добавлены `TriggerAuto` (`nny_boot` через 1 с) и `EnableDisableStreams`; коллизии нужен материал поверхности 18, чтобы держать машину; цена — около 3 МБ памяти.

## Что неизвестно

- Модель большой карты «Взгляда дона» (остров мода на ней не виден).
- Смысл атрибутов `ZoneWorld` и `ZoneRenderLOD.1` (похоже на радиусы загрузки и выгрузки зон).

## Подробнее

- [Уровни](../../levels.md) §1, §3, §10, §11.1
- [Нью-Йорк: устройство карты](../../analysis/queens.md) §1–§5, §7, §8
- [Живая связь](../../analysis/live-link.md) §13.1–13.3
