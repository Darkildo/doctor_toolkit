---
type: system
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 73, 23, 48, 63]
sources: ["docs/analysis/live-link.md §13.5", "docs/analysis/tunables.md §2, §3", "docs/analysis/queens.md §4, §8", "docs/formats/mission-logic.md §5.2", "docs/analysis/game-bench.md §31.3", "data/gf2/enums.toml"]
tags: [crew, family, player, live-link, upgrades]
updated: 2026-11-16
---
# Команда игрока (семья)

Команда игрока — члены его семьи (`corleone`). Известно устройство списка команды в памяти (живая связь, только чтение) и несколько параметров в данных; найм и поведение команды не разобраны.

## Что известно

- Команда — `*0x0112989C` (`AddPlayerFamilyMember` `0x0090A370`, фабрика `0x006844C0`): 7 мест по 0x24 байта с `+0x18` (`0x0090B0A0`).
- Поле места `+0` — флаги: бит 0 — место открыто, бит 1 — занято (`0x0090B690`).
- `+4` — ранг, который просит место: 3, 3, 3, 3, 4, 4, 5 (солдат, капо, младший босс).
- `+8` — ручка члена команды; член — ручка − 0x48 (`0x0090A5B0`), объект SimGroup (GUID `+0x20`); его ранг — `+0xD0` (`0x0079179B`; пишет `0x00791670`).
- `+0x10` — специальности (`0x0090A5C5`): 0x04 подрывник, 0x08 поджигатель, 0x10 медвежатник, 0x20 инженер, 0x40 медик, 0x80 громила. Функция `0x009630E0` проверяет биты в этом порядке и возвращает ключ текста `$crew_specialty_demolition`, `_arsonist`, `_safe_cracker`, `_engineer`, `_medic`, `_bruiser`; в типах это перечисление [`crew_specialty`](../enums/crew_specialty.md).
- Имя члена команды клиент живой связи берёт по GUID из SimGroup (`live family --names`). На стенде после `-stream` все 7 мест закрыты.
- Параметры в данных (`gf2/globals_global`, см. [параметры](parameters.md)): `crew.corleone_associates.health` — `StandardDamageComponent` 0 = 2600 у 200 шаблонов `CommonNPC` с моделью `corleoneassociate_*` (что это охрана предприятий игрока — предположение).
- Бонусы криминальных колец для команды — множители [UpgradeData](../classes/UpgradeData.md): `Bullet Proof Vests` 14 = 0.8, `Armor Piercing Bullets` 16 = 1.25, `Ammo Holsters` 17 = 1.5, `Brass Knuckles` 18 = 1.25, `Jail Break` 13 = 0.5 (все — probably; см. [улучшения](upgrades.md)).
- Навыки члена семьи видны в меню Don's View → Family Tree → Upgrade; описание навыка подставляет значение атрибута `UpgradeData`.
- Речь команды — события таблицы `get` (например `CREW_IM_DOWN_MEDIC`), см. [диалоги](dialogues.md).
- «Люди чести» чужих семей — отдельные `CommonNPC` и `SimNpc` в `globals_global` (древо задания 3 показывает дона Кармине и четырёх), см. [семьи](families.md).

## Что неизвестно

- Запись команды (добавление NPC, `0x0090B4E0`) не проверена и модулем не делается.
- Как игра открывает места и выбирает кандидатов; шаблоны нанимаемых людей в данных не выделены (имён вида `NPC_CRW_*` в документах и данных репозитория нет).
- На что именно действует множитель `Jail Break`; значения навыков Specialty Focus (24 = 0.75/0.5) и других видны, но что они меняют, по данным не определить.

## Подробнее

- [Живая связь](../../analysis/live-link.md) §13.5, §13.9
- [Параметры игры](../../analysis/tunables.md) §2 «Здоровье и экипаж», §3
- [Нью-Йорк: устройство карты](../../analysis/queens.md) §4, §8
- [Стенд проверки в игре](../../analysis/game-bench.md) §31.3
