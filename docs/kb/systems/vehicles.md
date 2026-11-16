---
type: system
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 23, 42, 58, 59, 60, 63]
sources: ["docs/analysis/tunables.md §2 «Машины», §3", "docs/analysis/performance.md §6, §9, §10.7", "docs/analysis/queens.md §2, §7, §8", "docs/formats/navigation-graph.md §5", "docs/formats/mission-logic.md §2.3", "data/gf2/sim_classes.txt"]
tags: [vehicles, traffic, physics, tunables, navigation]
updated: 2026-11-16
---
# Транспорт и трафик

Шаблоны машин — объекты SimGroup в `gf2/globals_global`; трафиком города управляет `CarTrafficManager`; машины ездят по графам `graph` типа «машины».

## Что известно

- 50 объектов [AICarBehavior](../classes/AICarBehavior.md) в `globals_global` — шаблоны машин: в каждом пакеты `BaseVehicle`, `VehicleDamageComponent`, `WhiteboxCar`, аудио и пакеты физики (`DefaultEngine`, `DefaultTransmission`, `DefaultBrake`… и безымянный `34f699a5`). Модель — `Entity.CMD_SetModel` → metamodel в потоке `gf2/globalfolder/vehicles/assembly/vehicle_shared/<модель>`.
- Здоровье (`StandardDamageComponent` 0 = `CMD_SetInitialHealth`): спорткары 12500; седаны, такси, универсал 15000; muscle и полицейские машины 17500; фургоны и `thug60` 20000; `thug60_armored` 22500; пикапы 25000; автозаки (`police_wagon`, `truck_heavy_02_military`) 27500; тяжёлые грузовики 30000.
- `VehicleDamageComponent` 14 — «насколько сильно машина получает урон» (probably): 0.6 спорткары, 0.55 седаны, 0.5 джип, 0.4 машины семей и фургоны, 0.3 пикапы, 0.2 автозаки, 0.1 бронированная машина семьи и тяжёлые грузовики. Бензовоз `truck_heavy_fuel` — 15000 и 0.1.
- Машина семьи — `Family` 6 → `thug60`; улучшение `Armored Thug Car` (`UpgradeData` 11) → `thug60_armored`; полицейские машины — `CrimeDefinitions` 15 (см. [полиция](police.md)).
- Пакеты физики с неназванными атрибутами: [DefaultTransmission](../classes/DefaultTransmission.md) (`e9613cc5`) — обороты 4000/6500 и передаточные числа 3.21/2.176/1.475/1.0; [DefaultEngine](../classes/DefaultEngine.md) (`50f03f03`) 4 (175–400); безымянный [`34f699a5`](../classes/class-34f699a5.md) 0 (750, 1500, 2500, 4500 — похоже на массу).
- `CarTrafficManager` — по одному на город (`…\local`): `TrafficManager.11` = 10, 10 и 14 (похоже на число машин), `.3` = 60/95/105 и `.4` = 120/160 (радиусы появления и исчезновения), `.16`–`.18` — сообщения `cars_tm_disable_spawns`, `cars_tm_enable_spawns`, `cars_tm_despawn_all`.
- Опыт в Нью-Йорке: `cars_tm_disable_spawns` и `cars_tm_despawn_all` убрали 5 из 11 машин в 200 м (остальные 6 — припаркованные); на время кадра это почти не влияет.
- Граф дорог — `graph` с типом `0x04` (машины, 91 граф) и `0x10` (парковка, 853); см. [навигационный граф](../engine/navigation-graph.md).
- Материал коллизии решает, держит ли она машину: поверхность 11 (образец-дерево) — машина проваливается до днища, поверхность 18 (земля дока) — стоит (проверено в игре).
- Наблюдение на стенде: машины задания (`AICarBehavior`) есть не в каждом запуске и уходят после смерти игрока.
- Lua: `GetVehicleType`, `GetVehicleSpeed`, `GetVehicleHealth`, `GetVehicleSkidEnergy` и др. (звук машин).

## Что неизвестно

- Скорость и масса машин: безымянные классы без имён атрибутов, в список параметров не включены.
- Смысл `TrafficManager.*`: читатели не разобраны; правка `.11` (машины 28 и 4 вместо 14) число `AICarBehavior` у игрока не изменила.
- Машинный граф для новой суши не сделан: машины трафика на остров мода не заезжают; инструментов для машинного графа нет.

## Подробнее

- [Параметры игры](../../analysis/tunables.md) §2 «Машины», §3
- [Производительность](../../analysis/performance.md) §6, §9, §10.7
- [Нью-Йорк: устройство карты](../../analysis/queens.md) §2, §7, §8
- [Навигационный граф](../../formats/navigation-graph.md) §5
