---
type: system
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 73, 23, 48, 63, 82]
sources: ["docs/analysis/crew.md", "docs/analysis/game-bench.md §44", "research/crew_survey.py", "crates/doctor-toolkit/tests/corpus/crew.rs", "docs/analysis/live-link.md §13.5", "docs/analysis/tunables.md §2, §3", "data/gf2/class_behavior.toml", "data/gf2/enums.toml"]
tags: [crew, family, player, made-men, recruits, live-link, upgrades]
updated: 2026-11-16
---
# Команда игрока (семья)

У игрока два списка: **семья** — до семи людей чести в древе (4 солдата, 2 капо, младший босс), и **команда** — до трёх из них, идущих за ним. Данные, найм, повышение, гибель, лицензии и дополнения разобраны; нового человека можно сделать одними данными ([пример](../experiments/new-crew-member.md)). Не решён его портрет.

## Что известно

- Семья — объект `*0x0112989C`: семь мест по 0x24 байта с `+0x18`: флаги (бит 0 открыто, бит 1 занято), ранг места (3, 3, 3, 3, 4, 4, 5), ручка `SimNpc`, специальности, GUID оружия ([адреса](../exe/player-crew.md)).
- Рекруты — 81 шаблон `CommonNPC` с именем `NPC_CRW_0001…0081` в атрибуте `NPC.2` (`gf2/globals_global`), у каждого свой [SimNpc](../classes/SimNpc.md). Хэш текста имени (`NPC.18`) — `sdbm("name_" + NPC.2)`; биография — текст `<имя>_background`.
- Специальности — биты `0x04`…`0x80` слова [NPCCrewComponent](../classes/NPCCrewComponent.md).0 ([crew_specialty](../enums/crew_specialty.md)); внешность — набор, пресет и элемент пресета базы частей `player` (`PartedAnimated`).
- Места открывает сюжет: восемь сообщений `iMsgTreeType…` поднимают уровень древа 0…8 (`0x0090BA70`); уровень пишется в сохранение.
- Найм: рекрут стоит в мире ([SpecificNpcRequester](../classes/SpecificNpcRequester.md) + `SpawnPoint`, 80 мест в трёх городах), игрок нанимает его разговором, если есть открытое свободное место солдата (`0x0090B220`). Объект [AddPlayerFamilyMember](../classes/AddPlayerFamilyMember.md) принимает человека сразу и сам открывает место.
- Повышение — кнопка «Promote»: человек переходит в место старшего ранга и получает ещё одну специальность (`0x0090B330`, [ранги](../enums/crew_rank.md)).
- Раненый возвращается; навсегда убирает только приговор «Mark for Death» (бит 29 флагов `SimNpc`, `0x0090B820`).
- Оружейная лицензия (1 Basic … 4 Master) — байт записи заслуг человека: начальная — по флагам `SimNpc`, растёт с заслугами 10, 25, 70 ([weapon_licence](../enums/weapon_licence.md), [сохранения](saves.md)).
- Рекруты дополнений (Tommy Cipolla, Jimmy Lira и семь запасных шаблонов) лежат в тех же данных и закрыты объектами [UnlockCheck](../classes/UnlockCheck.md) по флагу в объекте денег игрока.
- Параметры: `crew.corleone_associates.health` (200 шаблонов `corleoneassociate_*`), множители [UpgradeData](../classes/UpgradeData.md) для команды — см. [параметры](parameters.md), [улучшения](upgrades.md); речь — события `CREW_*` ([диалоги](dialogues.md)).

## Что неизвестно

- Почему у нового человека портрет по умолчанию: [вопрос](../questions/crew-portrait-of-new-member.md).
- Специальность в деле (`CSACrewComponent`), таймеры больницы и тюрьмы, кто шлёт `iMsgCrewDeath`.
- Биты 0 и 8 слова `NPCCrewComponent.0`, атрибуты `SimNpc` 2–7.
- На что действуют множители `Jail Break` и Specialty Focus.

## Подробнее

- [Команда: люди чести семьи игрока](../../analysis/crew.md)
- [Стенд проверки в игре](../../analysis/game-bench.md) §44, §31.3
- [Живая связь](../../analysis/live-link.md) §13.5, §13.9
- [Этап 82](../research/stage-82.md)
