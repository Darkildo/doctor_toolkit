# Параметры игры для мастера «Изменить параметры игры»

Список `data/gf2/tunables.toml` — 75 параметров GF2 PC (оружие, здоровье,
полиция, машины, улучшения, экономика), которые мастер этапа 23 даёт
менять без знания форматов. Все параметры — атрибуты объектов SimGroup
`gf2/globals_global:simgroup:00000000` (поток загружается всегда). Список
собирает и проверяет `research/tunables.py`.

## 1. Как построен список

Имена атрибутов SimGroup в исполняемом файле не хранятся (известны ~40,
`simgroup.md` §4), поэтому каждый параметр задан как «класс пакета + индекс
атрибута + объекты». Смысл выведен из данных, а не из имени:

1. **Выбор объектов не зависит от описываемого атрибута.** Оружие игрока —
   по GUID, на которые ссылается объект `Player` (атрибуты 113–145 пакета
   `Player`: 20 `Gun`, бита, удавка, молотов, динамит, бомба); улучшения —
   по имени в атрибуте 0 `UpgradeData` (`Heart Level 1`, …); семьи — по
   имени в атрибуте 1 `Family`; NPC — по модели персонажа
   (`PartedAnimated` 2, `crosato_0001_main`, `nyc_police_03`, …); машины —
   по модели (`Entity.CMD_SetModel` → metamodel в потоке
   `gf2/globalfolder/vehicles/assembly/vehicle_shared/<модель>`, найдено
   `doctor-toolkit search type:metamodel stream:vehicles`); погони — по
   цепочке `Family` (`miami_law`, `queens_law`, `cuban_law`) атрибут 7 →
   `ChaseConfigList` → четыре `ChaseConfig`.
2. **Смысл атрибута — по закономерности значений** между объектами одного
   класса: растёт с уровнем оружия, совпадает с названием улучшения,
   отличается только у «бронированной» машины и т. п. Где закономерность
   объясняет значение не однозначно, описание говорит «probably».
3. **Имя атрибута из metamodel** — только `StandardDamageComponent` 0 =
   `CMD_SetInitialHealth` (здоровье игрока, NPC и машин); это самые надёжные
   записи.
4. **Сторонние подтверждения**: таблица Cheat Engine из папки игры
   (`scripts/The Godfather II®.CT`) знает у объекта оружия поля «Damage» и
   «Rate Of Fire» рядом в памяти (смещения 0x474/0x478) и у семьи поля
   «Minimum/Maximum time before next turn» — это подтверждает, что такие
   параметры существуют, но связи «смещение в памяти → индекс атрибута» нет.
   Ghidra (MCP) в этой сессии была недоступна (соединение отклонено).

`tunables.py build` собирает TOML из списка в коде, `check` проверяет
каждую запись: ресурс есть, все объекты есть и единственны в группе, у
каждого есть пакет класса, атрибут хранится и имеет вид `kind`, значение в
пределах `min`/`max`; печатает текущие значения и есть ли тот же GUID в
других потоках. Результат: 75 записей, 0 ошибок, ни один выбранный объект
не повторяется в других потоках (копии с тем же GUID в миссиях не нужно
править).

```sh
doctor-toolkit logic-export --type simgroup -o <exp> <игра>
doctor-toolkit search --limit 0 --keys <игра> type:metamodel stream:vehicles > vehicles.txt
python3 -I research/tunables.py build <exp> vehicles.txt > data/gf2/tunables.toml
python3 -I research/tunables.py check <exp> data/gf2/tunables.toml
python3 -I research/tunables.py survey <exp>     # таблицы доказательств ниже
```

Записи оружия покрывают все четыре уровня улучшения одним значением
(`objects` — четыре `Gun`); текущие значения по уровням — в таблице.

## 2. Записи

Ресурс у всех — `gf2/globals_global:simgroup:00000000`. «Значения» —
текущие, по объектам записи. Уверенность: **имя** — атрибут назван
metamodel; **сильная** — однозначная закономерность; **probably** —
описание помечено как предположение.

### Оружие (27)

Оружие игрока: `Player` 114/132/137/142 → пистолет ур. 1–4, 115/133/138/143
→ магнум, 116/134/139/144 → автомат Томпсона (`weapon_automatic_*`),
117/135/140/145 → дробовик, 113/131/136/141 → винтовка.

| id | класс.атр | значения (ур. 1–4) | доказательство | уверенность |
|---|---|---|---|---|
| `weapons.pistol.damage` | Gun.6 | 700, 800, 900, 1200 | растёт с уровнем; магнум 2000–4000, винтовка 2750–5000, автомат 235–440 — порядок по силе оружия; урон дробовика 150–275 при 10 дробинах; **игра**: автомат ур. 1 с 20000 поджёг машину вторым патроном (с 235 — 100 патронов без пожара), `game-bench.md` §31.1 | **проверено** |
| `weapons.magnum.damage` | Gun.6 | 2000, 2200, 3000, 4000 | то же | сильная (атрибут проверен) |
| `weapons.tommy_gun.damage` | Gun.6 | 235, 275, 325, 440 | то же | **проверено** |
| `weapons.shotgun.damage` | Gun.6 | 150, 180, 240, 275 | то же, урон одной дробины | сильная (атрибут проверен) |
| `weapons.rifle.damage` | Gun.6 | 2750, 3300, 4400, 5000 | то же | сильная (атрибут проверен) |
| `weapons.*.clip_size` | Gun.7 | пистолет 8,7,7,10; магнум 6; автомат 40,50,60,75; дробовик 1,2,5,6; винтовка 1,5,5,6 | целое, порядок величин магазинов (револьвер 6, Томпсон 40–75); **игра**: 13 у автомата и 3 у пистолета — счётчик «13-39», «3-9» | **проверено** |
| `weapons.*.max_ammo` | Gun.8 | пистолет 32,35,42,50; магнум 18,24,30,36; автомат 200,250,300,400; дробовик 8,12,25,30; винтовка 8,20,18,25 | целое, кратно магазину, растёт с уровнем; **игра**: при 52 и 12 выданные патроны срезаны до 52 и 12 (без правки выдача — 100 и 16, меньше предела) | **проверено** |
| `weapons.*.fire_interval` | Gun.15 | пистолет 0.15; магнум 0.6; автомат 0.09,0.08,0.07,0.06; дробовик 0.6; винтовка 0.75,0.675,0.6,0.5 | секунды между выстрелами: у автомата меньше всех и падает с уровнем | probably |
| `weapons.shotgun.pellets` | Gun.25 | 10 ×4 | 10 у всех вариантов дробовика в корпусе, 1 у всего остального оружия | probably |
| `weapons.molotov.damage` | ThrowableItem.7 | 2500 | молотов 2500 < динамит 5000 < бомба 7500 | probably |
| `weapons.dynamite.damage` | ThrowableItem.7 | 5000 | то же | probably |
| `weapons.bomb.damage` | ThrowableItem.7 | 7500 | то же | probably |
| `weapons.molotov.radius` | ThrowableItem.6 | 5 | пара 5/6 = 2.5/5, 5/7.5, 7.5/10 — внутренний и внешний радиус | probably |
| `weapons.dynamite.radius` | ThrowableItem.6 | 7.5 | то же | probably |
| `weapons.bomb.radius` | ThrowableItem.6 | 10 | то же | probably |

`weapons.*` — по четыре записи на каждый из пяти видов оружия (урон,
магазин, запас, интервал); в таблице они сведены по атрибутам.

**Копии в атрибутах 88–99.** Блоки `88..91`, `92..95`, `96..99` повторяют
(урон, интервал, магазин, запас) из атрибутов 6, 15, 7, 8. Во всём корпусе
(429 `Gun`) урон 6 отличается от копий у 37 объектов — все это винтовка
ур. 1 из набора мультиплеера (6 = 2950, копии 2750): баланс правили только
в атрибуте 6. Интервал 15 отличается от копий у 134 объектов (у винтовок
игрока 15 = 0.75/0.675, копия 89 = 1.0). Поэтому записи используют 6, 15, 7,
8, а копии считаем, вероятно, неиспользуемыми; в игре это не проверено.
Атрибуты 53–87 (пять блоков по 7) похожи на параметры оружия в руках NPC
(урон 55: пистолет 325–600, вдвое меньше, чем у игрока) — не включены.

В `globals_global` есть второй набор из 20 `Gun` с теми же именами (GUID
`64f7c858…`), он же лежит на картах мультиплеера; игрок на него не
ссылается. Оружие, разложенное в миссиях (`…/compoundstream`, хит-миссии),
— отдельные объекты со своими значениями; какие значения действуют после
подбора такого оружия, не проверялось.

### Здоровье (4) и экипаж (6)

| id | класс.атр | объекты | значения | доказательство | уверенность |
|---|---|---|---|---|---|
| `health.player.health` | StandardDamageComponent.0 | `Player` | 4500 | `CMD_SetInitialHealth` | имя |
| `health.carmine_soldiers.health` | StandardDamageComponent.0 | 200 `CommonNPC`, модели `crosato_NNNN_*` с 1800 | 1800 | имя атрибута; капо и солдаты Кармине (3500, 4250) не входят | имя |
| `health.tony_soldiers.health` | то же | 200, `trosato_*` с 2000 | 2000 | то же | имя |
| `health.granados_soldiers.health` | то же | 200, `granadosassoc*` | 2200 | то же | имя |
| `crew.corleone_associates.health` | то же | 200, `corleoneassociate_*` | 2600 | имя атрибута; что это охрана предприятий игрока — предположение | имя / probably |
| `crew.bullet_proof_vests.multiplier` | UpgradeData.14 | `Bullet Proof Vests` | 0.8 | единственное отличие объекта от 1.0, совпадает с названием бонуса | probably |
| `crew.armor_piercing_bullets.multiplier` | UpgradeData.16 | `Armor Piercing Bullets` | 1.25 | то же | probably |
| `crew.ammo_holsters.multiplier` | UpgradeData.17 | `Ammo Holsters` | 1.5 | то же | probably |
| `crew.brass_knuckles.multiplier` | UpgradeData.18 | `Brass Knuckles` | 1.25 | то же | probably |
| `crew.jail_break.multiplier` | UpgradeData.13 | `Jail Break` | 0.5 | то же; на что именно множитель — предположение | probably |

Общее у `UpgradeData`: атрибут 0 — имя, 3 — категория (4 — бонусы
криминальных колец, 2 и 1 — навыки), 4 — номер в списке (бонусы колец
идут подряд 50–57, у уровней одного навыка одинаков — это не цена), 5 —
уровень, атрибуты 12–62 — множители, равные 1.0 у всех улучшений, кроме
того, к которому множитель относится.

### Игрок (11)

| id | класс.атр | объект | значение | доказательство | уверенность |
|---|---|---|---|---|---|
| `player.carry_more_explosives.max_molotovs` | UpgradeData.21 | `Carry More Explosives` | 8 | у всех прочих улучшений 21/22/23 = 4/3/1, у этого 8/6/2; порядок как у ссылок игрока (119 молотов, 120 динамит, 121 бомба) | probably |
| `player.carry_more_explosives.max_dynamite` | UpgradeData.22 | то же | 6 | то же | probably |
| `player.carry_more_explosives.max_bombs` | UpgradeData.23 | то же | 2 | то же | probably |
| `player.health_regen_1.multiplier` | UpgradeData.31 | `Health Regen Level 1` | 1.25 | единственное отличие от 1.0, растёт с уровнем; меню навыков игры: «Recover health 25% faster» (атрибут 2 — ключ этой строки `8214343b`) | **проверено** |
| `player.health_regen_2.multiplier` | UpgradeData.31 | `Health Regen Level 2` | 1.5 | то же | probably |
| `player.quick_heal.multiplier` | UpgradeData.12 | `Quick Heal` | 0.5 | то же | probably |
| `player.heart_1.bonus` | UpgradeData.35 | `Heart Level 1` | 10 | **секунды жизни после ранения** (навык Survival: «Stay alive longer after being punched or gunned down. Gives a medic more time to make a revival»; атрибут 1 — имя `41330554` «Survival», 2 — описание `6e269595` «Extend survival by {n} seconds»); **игра**: 17 → «Extend survival by 17 seconds» | **проверено** |
| `player.heart_2.bonus` | UpgradeData.35 | `Heart Level 2` | 20 | то же, секунды | сильная (атрибут проверен) |
| `player.heart_3.bonus` | UpgradeData.35 | `Heart Level 3` | 30 | то же, секунды | сильная (атрибут проверен) |
| `player.heavy_hitter_1.multiplier` | UpgradeData.32 | `Heavy Hitter Level 1` | 1.25 | 1.25/1.5 по уровням | probably |
| `player.heavy_hitter_2.multiplier` | UpgradeData.32 | `Heavy Hitter Level 2` | 1.5 | то же | probably |

### Экономика (4)

| id | класс.атр | объекты | значения | доказательство | уверенность |
|---|---|---|---|---|---|
| `economy.reduced_law_costs.multiplier_a` | UpgradeData.19 | `Reduced Law Costs` | 0.5 | два множителя 0.5 у бонуса «меньше расходов на закон»; какой расход какой — неизвестно | probably |
| `economy.reduced_law_costs.multiplier_b` | UpgradeData.20 | то же | 0.5 | то же | probably |
| `economy.banks.cash_a` | Bank.0 | 4 `Bank` (`bank_26/28/29/31_pickup`) | 10000 ×4 | целые денежного вида у объектов «банк» | probably, слабая |
| `economy.banks.cash_b` | Bank.1 | то же | 5000 ×4 | то же | probably, слабая |

### Полиция (6)

Погоня: `Family` `miami_law`/`queens_law`/`cuban_law` атрибут 7 →
`ChaseConfigList` → четыре `ChaseConfig` с атрибутом 16 = 1, 2, 3, 4
(уровень). Значения у трёх городов одинаковы:

| уровень | 2 | 3 | 7 | 8 | 10 | 11 |
|---|---|---|---|---|---|---|
| 1 | 20 | 1 | 1 | 7 | — | 75 |
| 2 | 40 | 2 | 2 | 5 | 75 | 150 |
| 3 | 60 | 3 | 3 | 1 | 150 | 225 |
| 4 | 80 | 3 | 3 | — | 225 | 1000 |

Атрибуты 10/11 — смыкающиеся интервалы (0–75, 75–150, 150–225,
225–1000), это границы «жара» уровней розыска. Атрибут 9 — список машин
(`VehicleList`, на уровне 4 добавлен `police_wagon`), 12 — `Gun`, который
выдаётся полицейским (Майами и Нью-Йорк — дробовик ур. 1, Куба — Томпсон
ур. 1); GUID-значения в мастер не входят.

| id | класс.атр | объекты | значения | уверенность |
|---|---|---|---|---|
| `police.wanted_2.heat_threshold` | ChaseConfig.10 | уровень 2 трёх городов | 75 ×3 | probably (сильная закономерность) |
| `police.wanted_3.heat_threshold` | ChaseConfig.10 | уровень 3 | 150 ×3 | то же |
| `police.wanted_4.heat_threshold` | ChaseConfig.10 | уровень 4 | 225 ×3 | то же |
| `police.wanted_4.heat_max` | ChaseConfig.11 | уровень 4 | 1000 ×3 | probably |
| `police.officers.health` | StandardDamageComponent.0 | 65 `CommonNPC`: `miami_police_*`, `nyc_police_*`, `cuban_police_sergeant_*` | 2200 ×65 | имя |
| `police.sergeants_swat.health` | StandardDamageComponent.0 | 30: `miami/nyc_seargent_*`, `miami/nyc_swat_*` | 2750 ×30 | имя |

Порог уровня записан дважды (атрибут 11 нижнего уровня и 10 верхнего);
мастеру стоит менять их вместе.

### Машины (12)

50 объектов `AICarBehavior` в `globals_global` — шаблоны машин (в каждом
пакеты `BaseVehicle`, `VehicleDamageComponent`, `WhiteboxCar`, аудио, и
безымянные классы с передаточными числами `e9613cc5`: 3.21/2.176/1.475/1.0).
`CrimeDefinitions` 15 каждого города указывает на полицейскую машину
(`police_miami`, `police_queens`, `jeep60_01`), `Family` 6 — на `thug60`,
`UpgradeData` `Armored Thug Car` 11 — на `thug60_armored`.

| id | класс.атр | модели | значения | уверенность |
|---|---|---|---|---|
| `vehicles.sedans.health` | StandardDamageComponent.0 | sedan50_01/02, sedan60_01/02/03, taxi, wagon60_01 | 15000 ×7 | имя |
| `vehicles.sports_cars.health` | то же | sportscar_01/02 | 12500 ×2 | имя |
| `vehicles.muscle_cars.health` | то же | muscle60_01/02/03 | 17500 ×4 | имя |
| `vehicles.vans.health` | то же | van60_01/02 | 20000 ×2 | имя |
| `vehicles.pickups.health` | то же | truck_pickup, truck_pickup_stepside, truck_flatbed | 25000 ×3 | имя |
| `vehicles.heavy_trucks.health` | то же | truck_heavy_racket, …_queens, truck_heavy_tow | 30000 ×3 | имя |
| `vehicles.family_cars.health` | то же | thug60 | 20000 ×7 | имя |
| `vehicles.armored_family_cars.health` | то же | thug60_armored | 22500 ×4 | имя |
| `vehicles.police_cars.health` | то же | police_miami, police_queens, jeep60_01 | 17500 ×10 | имя |
| `vehicles.police_wagons.health` | то же | police_wagon, truck_heavy_02_military | 27500 ×6 | имя |
| `vehicles.family_cars.damage_factor` | VehicleDamageComponent.14 | thug60 | 0.4 ×7 | probably |
| `vehicles.armored_family_cars.damage_factor` | VehicleDamageComponent.14 | thug60_armored | 0.1 ×4 | probably |

`VehicleDamageComponent` 14: 0.6 спорткары, 0.55 седаны, 0.5 джип,
0.4 машины семей и фургоны, 0.3 пикапы, 0.2 автозаки, 0.1 бронированная
машина семьи и тяжёлые грузовики — чем крепче машина, тем меньше; поэтому
«насколько сильно машина получает урон». Бензовоз `truck_heavy_fuel`
(15000, 0.1) в группы не включён.

### Мир (5)

| id | класс.атр | объект | значение | уверенность |
|---|---|---|---|---|
| `world.rosato_carmine.strength` | Family.8 | `rosato_carmine` | 1.0 | probably |
| `world.rosato_tony.strength` | Family.8 | `rosato_tony` | 1.05 | probably |
| `world.granados.strength` | Family.8 | `granados` | 1.1 | probably |
| `world.mangano.strength` | Family.8 | `mangano` | 1.2 | probably |
| `world.almeida.strength` | Family.8 | `almeida` | 1.4 | probably |

У всех прочих 23 семей атрибут 8 = 1.0; у пяти соперников он растёт по
ходу сюжета, а атрибут 23 одновременно падает (60, 50, 40, 30, 20). Что
именно умножается — неизвестно.

## 3. Что осталось неизвестным

- **Доходы предприятий и рэкета, цены улучшений, взятки.** В SimGroup их
  нет: `Building` (113 объектов) хранит только id заведения, город, события
  и строки интерфейса; `JobReward` — иконки и события; `DynamicJob`,
  `HitMission` — события. Цены улучшений `UpgradeData` не хранит (атрибут 4
  — номер в списке). Вероятно, это в Lua или в коде игры.
- **Деньги с убитых NPC**: `NPC` 25/26 = 1/100 у прохожих, 25/50 у
  полиции и охраны, 1000/2000 у бандитов, 1500/3000 у особых — похоже на
  мин./макс. сумму, но не проверено и разбросано по ~3400 шаблонам.
- **Наличные на месте**: `Item` «Cash» атрибут 4 (100…4000) — вероятно, суммы
  подбираемых денег; объекты почти все в локальных потоках заведений; «Health Pack» 4
  (500, в мультиплеере 1500) — вероятно, сколько лечит. Не включены: по
  одному объекту на заведение.
- **Остальные навыки**: Handgun/Heavy Weapons Specialization (44 = 2/3/5 и
  47 = 0.9/0.8/0.7; 51 = 2/3/5 и 54 = 0.925/0.85/0.75), Marksman (45, 46 =
  0.6; 52, 53 = 0.75), Combat Reflexes (26, 33 = 1.25/1.5; 34 = 5/6),
  Ready Weapon (36–41 = 1.5), Extortion (30 = 30), Dangerous Appearance
  (25 = 0.5), Specialty Focus (24 = 0.75/0.5), Vice Grip (28 = 0.5) —
  значения видны, но что они меняют, по данным не определить.
- **Полиция**: `CrimeDefinitions` (по объекту на город, 205 атрибутов,
  22 повторяющихся блока по 8 — вероятно, виды преступлений),
  `CrimeManager` (пуст), `RoadblockManager` (50, 75, 155, 225, 105, 10 —
  похоже на пороги «жара», но не доказано), `ChaseConfig` 2/3/7/8 (растут
  с уровнем; возможно, число машин и полицейских), `ChaseManager`
  (146 атрибутов), `WitnessCrime` 3 (150).
- **ИИ**: `GlobalAIValueTuner` (0.5, 1.0), `NPCCombatConstants`
  (42 значения), `TargetingWeights` — без закономерностей, по которым можно
  назвать атрибут.
- **Семьи**: атрибуты 18–21 (`corleone` 720/720, соперники 240–360 и
  630–1310 секунд) — таблица Cheat Engine показывает у семьи таймеры
  «Minimum/Maximum time before next turn», но какой атрибут какой таймер,
  не установлено.
- **Машины**: скорость и масса. Безымянные пакеты `34f699a5` 0 (750, 1500,
  2500, 4500 — похоже на массу), `50f03f03` 4 (175–400), `e9613cc5`
  (обороты 4000/6500 и передаточные числа) — классы без имён, не включены.
- **metamodel и variabledictionary**: `CMD_SetInitialHealth` в metamodel
  задаёт здоровье разрушаемых частей конкретных моделей (бочки, окна) —
  для общего мастера не подходит; `variabledictionary` (26 ресурсов)
  хранит флаги сюжета и обучения (`global_tutorial_*`, `autosave_*`,
  `*_compound`), не параметры. Записей этих источников в списке нет.
- **Проверка в игре** (третья волна, `game-bench.md` §31): урон, магазин и
  запас оружия (`Gun` 6, 7, 8), Heart (`UpgradeData` 35 — секунды, а не
  проценты здоровья; подпись исправлена) и Health Regen (31) — проверены.
  Описание навыка в меню игры подставляет значение атрибута в строку,
  на которую указывает атрибут 2 `UpgradeData` («Extend survival by {n}
  seconds», «Recover health {n}% faster»): так проверяются и остальные
  навыки. **Не проверены**: `Family` 8 (сила семьи — ни в одном экране
  после старта `-stream` не видна; в данных она растёт по ходу сюжета)
  и `Reduced Law Costs` 19/20 — бонус криминального кольца: у объекта нет
  ключей строк (атрибуты 1, 2), в списке бонусов колец игры такого имени
  нет (`Cheaper Guards`, `Double Crime Ring Income`, `Bombed Buildings
  Rebuild Faster`, …), а кольцо со старта не получить; какой бонус кольца он
  даёт в игре и что умножает — открыто.