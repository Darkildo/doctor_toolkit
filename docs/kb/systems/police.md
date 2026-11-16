---
type: system
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 16, 23, 27, 28]
sources: ["docs/analysis/tunables.md §2 «Полиция», «Машины», §3", "docs/formats/mission-logic.md §2.3", "docs/logic-graph.md §1", "docs/scripts.md §9"]
tags: [police, chase, wanted, crime, tunables]
updated: 2026-11-16
---
# Полиция и закон

Параметры погони и розыска лежат в объектах SimGroup `gf2/globals_global`. Смысл выведен по закономерностям значений, в игре не проверялся.

## Что известно

- Цепочка погони: [Family](../classes/Family.md) `miami_law` / `queens_law` / `cuban_law`, атрибут 7 → `ChaseConfigList` → четыре [ChaseConfig](../classes/ChaseConfig.md) с атрибутом 16 = 1, 2, 3, 4 (уровень). Значения у трёх городов одинаковы.
- `ChaseConfig` по уровням 1–4: атрибут 2 = 20, 40, 60, 80; 3 = 1, 2, 3, 3; 7 = 1, 2, 3, 3; 8 = 7, 5, 1, —; 10 = —, 75, 150, 225; 11 = 75, 150, 225, 1000.
- Атрибуты 10/11 — смыкающиеся интервалы (0–75, 75–150, 150–225, 225–1000): границы «жара» уровней розыска. Порог записан дважды (11 нижнего уровня и 10 верхнего) — менять вместе.
- Атрибут 9 — список машин (`VehicleList`, на уровне 4 добавлен `police_wagon`); 12 — `Gun`, который выдаётся полицейским (Майами и Нью-Йорк — дробовик ур. 1, Куба — Томпсон ур. 1).
- `CrimeDefinitions` 15 каждого города указывает на полицейскую машину: `police_miami`, `police_queens`, `jeep60_01`.
- Здоровье (`StandardDamageComponent` 0): 2200 у 65 шаблонов `miami_police_*`, `nyc_police_*`, `cuban_police_sergeant_*`; 2750 у 30 шаблонов `miami/nyc_seargent_*`, `miami/nyc_swat_*`; полицейские машины 17500, автозаки 27500 (см. [транспорт](vehicles.md)).
- В список параметров входят 6 записей: `police.wanted_2/3/4.heat_threshold` (`ChaseConfig` 10), `police.wanted_4.heat_max` (11), здоровье полицейских и сержантов/SWAT.
- Lua видит полицию функциями `GetFootPoliceIntensity`, `GetCarPoliceIntensity`, `GetChaseVehicleStats`, `SetChaseVehicleSound`; шина знает сообщение `iMsgChaseStarted`.

## Что неизвестно

- `CrimeDefinitions`: по объекту на город, 205 атрибутов, 22 повторяющихся блока по 8 — вероятно, виды преступлений; не разобраны.
- `CrimeManager` пуст; `RoadblockManager` (50, 75, 155, 225, 105, 10) — похоже на пороги «жара», не доказано.
- `ChaseConfig` 2/3/7/8 растут с уровнем — возможно, число машин и полицейских.
- `ChaseManager` (146 атрибутов) и `WitnessCrime` 3 (150) — без названий.
- Взятки и расходы на закон: в SimGroup не найдены (см. [экономика](economy.md)).
- Функции полиции в Lua в игре не проверены (вне машины и без погони).

## Подробнее

- [Параметры игры](../../analysis/tunables.md) §2 «Полиция», §3
- [Логика миссий и Lua](../../formats/mission-logic.md) §2.3
- [Визуальные скрипты](../../scripts.md) §9
