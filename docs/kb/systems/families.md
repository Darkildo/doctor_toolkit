---
type: system
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 73, 23, 48, 60, 63]
sources: ["docs/analysis/tunables.md §1, §2 «Мир», «Полиция», §3", "docs/analysis/live-link.md §13.4", "docs/analysis/queens.md §4, §8", "data/gf2/enums.toml", "data/gf2/attr_types.toml", "docs/analysis/game-bench.md §53", "docs/formats/bricknmortarres.md"]
tags: [families, war, family, simgroup, tunables]
updated: 2026-11-18
---
# Семьи и война семей

Семья — объект SimGroup класса [Family](../classes/Family.md) в `gf2/globals_global`. В игре 28 семей: игрок (`corleone`), пять соперников и прочие, включая «семьи закона» городов. Полный список с хэшами — перечисление [`family`](../enums/family.md).

## Что известно

- Атрибут 1 `Family` — имя; в памяти имя семьи `+0x54` — `sdbm(lower(атрибут 1))`, список семей — `*0x01129904` (`+0x24` список, `+0x28` число), поиск `0x008C74D0`. `corleone` — хэш `0x0637B907`.
- Атрибут 8 — «сила семьи» (probably): `rosato_carmine` 1.0, `rosato_tony` 1.05, `granados` 1.1, `mangano` 1.2, `almeida` 1.4; у всех прочих 23 семей — 1.0. У пяти соперников он растёт по ходу сюжета, а атрибут 23 одновременно падает (60, 50, 40, 30, 20).
- Атрибуты 18–21 — таймеры: `corleone` 720/720, соперники 240–360 и 630–1310 секунд. Таблица Cheat Engine из папки игры показывает у семьи поля «Minimum/Maximum time before next turn».
- Атрибут 6 указывает на машину семьи `thug60`; атрибут 7 у `miami_law`, `queens_law`, `cuban_law` — на `ChaseConfigList` (см. [полиция](police.md)).
- Деньги семьи — `Family+0x100`, книги доходов — `Family+0x230` (см. [экономика](economy.md)).
- Принадлежность NPC семье — фильтры `GenomeFilterEntity` (атрибут 3 — хэш семьи: `529e27ee` у Кармине, `5c8559bf` у Тони).
- Хэш семьи `sdbm(lower(имя))` лежит в 15 атрибутах (этап 73, все значения найдены в списке семей): `Sentient` 2 у 3 433 персонажей, `GenomeFilter` 3 и 4, `Building` 4 (владелец заведения), `TestBuildingOwnership` 1, `ChangeBuildingOwnership` 2, `ChangeFamilySettings` 0, `TestFamilySettings` 0, `ChaseConfig` 0, `CrimeDefinitions` 1, `TestUpgrade` 0, `WhiteboxCar` 58 и `Family` 24–26 (другие семьи; смысл не разобран).
- Здоровье рядовых: Кармине (`crosato_NNNN_*`) 1800, Тони (`trosato_*`) 2000, Гранадос (`granadosassoc*`) 2200 — `StandardDamageComponent` 0 у 200 шаблонов `CommonNPC` на семью; солдаты и капо Кармине — 4250 и 3500.
- Смерть дона сообщает `StandardDamageComponent.5` его `CommonNPC` (`msg_don_killed_c_rosato` у Кармине; в данных его слушает только музыка).
- Захват заведения: `Building.25` — сообщение, которое здание шлёт, когда заведение переходит к игроку. У точек семей (тип 9) оно пустое, но если вписать своё, работает: проверено сменой владельца из логики (`ChangeBuildingOwnership`), стенд §53.
- Погоня `ChaseConfig` запускается своим сообщением (`ChaseConfig.4`) и вне задания. С семьёй Кармине (`ChaseConfig.0` = `529e27ee`) и её машиной `thug60_lincoln01` машины появляются, но стоят пустые (стенд §53).
- Заведения города и их исходные владельцы — ресурс [`bricknmortarres`](../../formats/bricknmortarres.md); новое заведение требует записи в нём (`venue add`).
- Укрепление семьи в мире: поток `homer/high/compoundstream` (Тони, заведение 51, 361 объект), появление бойцов фазами по `TriggerBox` и `TestBuildingOwnership`.

## Что неизвестно

- Что именно умножает атрибут 8; в игре он не проверен — после старта `-stream` сила семьи не видна ни на одном экране.
- Какой из атрибутов 18–21 какой таймер хода: связи «смещение в памяти → индекс атрибута» нет.
- Смысл атрибута 23.
- Логика ходов войны семей в документах не описана.

## Подробнее

- [Параметры игры](../../analysis/tunables.md) §1, §2 «Мир», §3
- [Живая связь](../../analysis/live-link.md) §13.4
- [Нью-Йорк: устройство карты](../../analysis/queens.md) §4, §8
- Связанные заметки: [команда игрока](crew.md), [параметры](parameters.md)
