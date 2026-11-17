---
type: system
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 16, 27, 28, 38, 48, 63, 76, 77]
sources: ["docs/analysis/story.md", "docs/analysis/game-bench.md §40", "docs/formats/mission-logic.md §1, §4, §6", "docs/logic-graph.md §2, §8, §9.2", "docs/analysis/live-link.md §13.6–13.7", "docs/analysis/queens.md §7", "docs/analysis/game-bench.md §30", "docs/scripts.md §1", "docs/analysis/tunables.md §3", "docs/formats/save.md §4"]
tags: [story, missions, checkpoints, simgroup, logic, variables]
updated: 2026-11-16
---
# Сюжет и миссии

Логика миссий — не Lua, а граф объектов SimGroup, связанных именами сообщений общей шины (хэш `sdbm(lower(имя))`, см. [шина сообщений](../engine/message-bus.md)). Сюжет — цепочка чекпоинтов; Lua 5.0.2 в игре — скриптовый движок звука и музыки, подключённый к той же шине ([Lua](../engine/lua.md)).

## Что известно

- **Чекпоинты.** В игре 166 объектов [Checkpoint](../classes/Checkpoint.md) в 69 главах; все лежат в корневых потоках городов (`gf2/queens` — 75, `gf2/miami` — 56, `gf2/cuba` — 27, `gf2/globals_global` — 8). Атрибуты: 1 — номер, 3 — глава, 2 — название главы, 5 — набор потоков, 7 `load`, 8 `on_loaded`, 9 `on_started`, 10 `on_restarted`, 14 — номер, под которым чекпоинт сохраняется. Таблица глав — [разбор](../../analysis/story.md) §4.
- **Жизнь чекпоинта** (`exe`, стенд): сообщение `load` делает чекпоинт текущим и запрашивает его потоки; по `iMsgStreamSetLoadComplete` рассылаются `iMsgNewCheckpointLoaded`, `…Started` или `…Restarted`, затем `on_loaded` и `on_started` либо `on_restarted`. Конец задания — сообщение `load` следующего чекпоинта.
- **Прогресс** — номер текущего чекпоинта: он пишется в сохранение, по нему игра возвращается в сюжет, его сравнивают 67 объектов [TestCurrentCheckpoint](../classes/TestCurrentCheckpoint.md) (чаще всего «не меньше 135»). Отдельной записи «миссия пройдена» нет.
- Менеджер чекпоинтов — `*0x011299C4` (конструктор `0x008FCCC0`): `+0x1C` — текущий `Checkpoint`, `+0x20`/`+0x24` — список и число чекпоинтов загруженных потоков; адреса — [чекпоинты и переменные](../exe/missions-variables.md), [классы сюжета](../exe/story-classes.md).
- **Цель на экране** — [UIHudObjective](../classes/UIHudObjective.md): очередь на 8 целей, одна текущая; её же текст показан вверху «Взгляда дона» (`ShowCurrentObjective`). Списка сюжетных заданий в интерфейсе нет: список во «Взгляде дона» — услуги ([UIJobInfo](../classes/UIJobInfo.md)), «History Log» меню паузы целей не содержит (стенд).
- **Награды**: события базы очков ([ExecuteScoreEvent](../classes/ExecuteScoreEvent.md), [JobReward](../classes/JobReward.md)), надпись [UIFanfare](../classes/UIFanfare.md), выдача предметов `GiveItemRelay`.
- **Провал**: [CheckpointFailure](../classes/CheckpointFailure.md) → `iMsgCheckpointFailed`, через 2 с — перезапуск. [MissionTimer](../classes/MissionTimer.md) (`0x0090EA70`): `start` — через `seconds − start_at` шлёт `on_expired`; `stop` — отмена и `on_stopped`; пауза и продолжение; с атрибутом 6 — таймер на экране.
- [MultiManager](../classes/MultiManager.md) слушает сообщение атрибута 0 и рассылает до 8 пар «сообщение, задержка»; триггеры (`TriggerAuto`, `TriggerBox`, `EnterExitTrigger`), логика (`AndEventGate`, `OrEventGate`, `VariableSwitch`, `VariableOperator`), задания (`HitMission` — 37 объектов, `DynamicJob` — 57), ролики (`TrinityGameSequence`, см. [ролики](cutscenes.md)).
- **Своё задание** собирается из копий объектов игры одной командой: `examples/quests/new-quest.sh` (чекпоинт, цель, метка, триггер, сохраняемая переменная, фанфара, автосохранение). На стенде проверены запуск, текущий чекпоинт, цель на экране и во «Взгляде дона», метка, триггер, переменная и запись чекпоинта и переменной в сохранение — [опыт](../experiments/story-bench.md).
- Свои узлы логики — ресурс `lua` и объект `LuaAudioScript`: запуск при загрузке и по сообщению, приём сообщений, `SendMessage` до объектов SimGroup, переменные и таймеры проверены в игре.
- Переменные сюжета — ресурсы `variabledictionary` (26 ресурсов). Менеджер — `*0x01223428`; переменная вида 1 пишется в [сохранения](saves.md) по хэшу записи. Объекты SimGroup обращаются к переменной по хэшу записи, Lua — по `sdbm` имени; `logic-import` добавляет в словарь новые переменные.
- Сообщение `cp_325_start_now` со старта `-stream` включает Миссию 3; вход в стартовый триггер у (232; 14; −345) переводит на чекпоинт 326. Падение `0x6E756F7B` в этом месте — не сюжет: [слишком длинный путь папки игры](../questions/crash-6e756f7b.md).

## Что неизвестно

- Загрузка сохранения с новым чекпоинтом, перезапуск после провала, «History Log», формат базы очков, поведение `HitMission` и `DynamicJob` — [вопрос](../questions/story-open-items.md).
- Смысл атрибутов 1–2 `MultiManager` и 6, 8–10 `TriggerBase`.
- Реакция большинства классов-действий в прогоне без игры — «чёрный ящик».

## Подробнее

- [Сюжет и миссии: разбор](../../analysis/story.md)
- [Стенд проверки в игре](../../analysis/game-bench.md) §30, §40
- [Логика миссий и Lua](../../formats/mission-logic.md) §1, §4, §6
- [Граф логики миссий](../../logic-graph.md) §1–§2, §8, §9
- [Живая связь](../../analysis/live-link.md) §13.6–13.7
- [Нью-Йорк: устройство карты](../../analysis/queens.md) §7
- [Визуальные скрипты](../../scripts.md) §1
