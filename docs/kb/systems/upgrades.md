---
type: system
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 23, 45, 75]
sources: ["docs/analysis/tunables.md §2 «Здоровье и экипаж», «Игрок», «Экономика», §3", "docs/analysis/game-bench.md §20.2, §31.3", "docs/analysis/handing-out.md §5"]
tags: [upgrades, skills, upgradedata, tunables, bench]
updated: 2026-11-16
---
# Навыки и улучшения

Навыки игрока и членов семьи и бонусы криминальных колец — объекты SimGroup класса [UpgradeData](../classes/UpgradeData.md) в `gf2/globals_global`.

## Что известно

- Общие атрибуты: 0 — имя (`Heart Level 1`, …), 1 и 2 — ключи строк имени и описания (у Survival: `41330554` и `6e269595` «Extend survival by {n} seconds»), 3 — категория (4 — бонусы криминальных колец, 2 и 1 — навыки), 4 — номер в списке (бонусы колец идут подряд 50–57; это не цена), 5 — уровень.
- Атрибуты 12–62 — множители, равные 1.0 у всех улучшений, кроме того, к которому множитель относится.
- Меню навыков игры подставляет значение атрибута в строку описания (атрибут 2) — так проверяются подписи.
- Проверено в игре: `Heart Level 1/2/3` — атрибут 35 = 10, 20, 30 — секунды жизни после ранения (навык Survival), а не проценты здоровья: при 17 меню показывает «Extend survival by 17 seconds». `Health Regen Level 1` — атрибут 31 = 1.25 — «Recover health 25% faster» (уровень 2 — 1.5).
- Игрок (probably): `Quick Heal` 12 = 0.5; `Heavy Hitter Level 1/2` 32 = 1.25/1.5; `Carry More Explosives` 21/22/23 = 8/6/2 (молотов, динамит, бомба; у прочих улучшений 4/3/1).
- Бонусы колец (probably): `Bullet Proof Vests` 14 = 0.8, `Armor Piercing Bullets` 16 = 1.25, `Ammo Holsters` 17 = 1.5, `Brass Knuckles` 18 = 1.25, `Jail Break` 13 = 0.5, `Reduced Law Costs` 19 и 20 = 0.5.
- `Armored Thug Car` — атрибут 11 указывает на машину `thug60_armored` (см. [транспорт](vehicles.md)).
- Путь к меню на стенде: `live money --add 500000`, Tab (Don's View) → Family Tree → член семьи → Upgrade.

- Список улучшений семьи — номера в `семья+0x124` (число `+0x128`); его проверяет [TestUpgrade](../classes/TestUpgrade.md) (`0x009B8E80`). Игроку улучшение выдаёт `0x007A6390` — только по сетевому запросу и из `DonControlGadget` сетевых карт (этап 75, по коду).
- «Улучшение оружия» — другое: оружие уровнем выше выдаётся `GiveItemRelay` с флагом 4 (см. [оружие](weapons.md)).

## Что неизвестно

- [Чем выдать навык в одиночной игре](../questions/upgrade-grant-single-player.md).

- Цены улучшений: `UpgradeData` их не хранит; вероятно, в Lua или в коде игры (см. [экономика](economy.md)).
- Остальные навыки — значения видны, но что они меняют, не определено: Handgun/Heavy Weapons Specialization (44 = 2/3/5 и 47 = 0.9/0.8/0.7; 51 = 2/3/5 и 54 = 0.925/0.85/0.75), Marksman (45, 46 = 0.6; 52, 53 = 0.75), Combat Reflexes (26, 33 = 1.25/1.5; 34 = 5/6), Ready Weapon (36–41 = 1.5), Extortion (30 = 30), Dangerous Appearance (25 = 0.5), Specialty Focus (24 = 0.75/0.5), Vice Grip (28 = 0.5).
- `Reduced Law Costs`: у объекта нет ключей строк (атрибуты 1, 2), в списке бонусов колец игры такого имени нет (`Cheaper Guards`, `Double Crime Ring Income`, `Bombed Buildings Rebuild Faster`, …); какой бонус он даёт и что умножает — открыто.
- На что именно действует множитель `Jail Break`.

## Подробнее

- [Параметры игры](../../analysis/tunables.md) §2, §3
- [Стенд проверки в игре](../../analysis/game-bench.md) §31.3
- [Выдача из логики: улучшения](../../analysis/handing-out.md) §5
- Связанные заметки: [команда игрока](crew.md), [параметры](parameters.md)
