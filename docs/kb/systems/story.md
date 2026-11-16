---
type: system
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 16, 27, 28, 38, 48, 63, 76]
sources: ["docs/formats/mission-logic.md §1, §4, §6", "docs/logic-graph.md §2, §8, §9.2", "docs/analysis/live-link.md §13.6–13.7", "docs/analysis/queens.md §7", "docs/analysis/game-bench.md §30", "docs/scripts.md §1", "docs/analysis/tunables.md §3", "docs/formats/save.md §4"]
tags: [story, missions, checkpoints, simgroup, logic, variables]
updated: 2026-11-16
---
# Сюжет и миссии

Логика миссий — не Lua, а граф объектов SimGroup, связанных именами сообщений общей шины (хэш `sdbm(lower(имя))`, см. [шина сообщений](../engine/message-bus.md)). Lua 5.0.2 в игре — скриптовый движок звука и музыки, подключённый к той же шине ([Lua](../engine/lua.md)).

## Что известно

- Потоки миссии: `gf2/<город>/global/objectives/<глава-миссия>/cp_<NNN>/...` (например `cuba/global/objectives/chapter60-mission08-killingcastro/cp_737`) и `gf2/globalfolder/checkpoints/...`; в Нью-Йорке задания — `global/checkpoints/c04…c66` (215 потоков). В корпусе 74 миссии.
- Единица логики — чекпоинт `cp_NNN`: классы [Checkpoint](../classes/Checkpoint.md), `TestCurrentCheckpoint`, `CheckpointFailure`. Атрибуты `Checkpoint`: 0 — заголовок, 1 — номер, 7 load, 8 on_loaded, 9 on_started, 10 on_restarted.
- Менеджер чекпоинтов — `*0x011299C4` (конструктор `0x008FCCC0`): `+0x1C` — текущий `Checkpoint`, его номер `+0x74`, глава `+0x70` (60 у `cp_735`, `cp_737`, `cp_739`); `+0x20`/`+0x24` — список и число чекпоинтов загруженных потоков. После `-stream gf2 cuba`: текущего нет, в потоках 34 чекпоинта.
- [MultiManager](../classes/MultiManager.md) слушает сообщение атрибута 0 и рассылает до 8 пар «сообщение, задержка» из атрибутов 3/4 … 17/18 (читатель `0x47A320`).
- Триггеры (`TriggerAuto`, `TriggerBox`, `TriggerRandom`, `EnterExitTrigger`), логика (`AndEventGate`, `OrEventGate`, `VariableSwitch`, `VariableOperator`, `VariableWatcher`, `MissionTimer`), действия (`FuncSpawn`, `SpawnPoint`, `GiveItemRelay`, `UIHudObjective`), задания (`HitMission` — 37 объектов, `DynamicJob` — 57, `JobReward`), ролики (`TrinityGameSequence`, см. [ролики](cutscenes.md)).
- `MissionTimer` (`0x0090DF30`): `start` — через `seconds − start_at` (атрибуты 3 и 9) шлёт `on_expired` (7); `stop` — отмена и `on_stopped` (8); пауза 10, продолжение 11.
- Свои узлы логики — ресурс `lua` и объект `LuaAudioScript`: запуск при загрузке и по сообщению, приём сообщений, `SendMessage` до объектов SimGroup, переменные и таймеры проверены в игре.
- Сообщение `cp_325_start_now` (`live send`) со старта `-stream` включает Миссию 3 «Укрепление Кармине»: текущий чекпоинт `cp_325`, экраны «Compound UNLOCKED», древо семьи, «Взгляд дона».
- Известное падение `0x6E756F7B`: вход в стартовый триггер Миссии 3 у (232; 14; −345) после `cp_325_start_now` со старта `-stream`, то же без мода. Быстрый путь годится для чекпоинтов, но не для прохождения заданий.
- Переменные сюжета — ресурсы `variabledictionary` (26 ресурсов: флаги сюжета и обучения `global_tutorial_*`, `autosave_*`, `*_compound`). Менеджер — `*0x01223428`: записи по 0x14 байт (`+4` — `sdbm(lower(имя))`, `+8` — значение `i32`, бит 0 в `+0xE` — переменная в [сохранениях](saves.md): это «вид 1» записи словаря, ключ в сохранении — хэш записи `+0`); на стенде вместимость 320, в Гаване 48 записей. Запись значения будит `VariableWatcher` (`0x00442240`).
- Проверено на стенде: `Use_Surveys = 1` и сообщение `check_should_use_surveys` — `VariableSwitch` из `gf2/globals_global` шлёт `do_survey_message`, игра показывает окно опроса.

## Что неизвестно

- Переход на чекпоинт (загрузка его потоков и логика миссии) через живую связь не проверен; менеджер только читается.
- Причина падения `0x6E756F7B`.
- Смысл атрибутов 1–2 `MultiManager` (по коду: 1 — число срабатываний, 2 — флаги) и 6, 8–10 `TriggerBase`.
- Отправитель сообщения в игре (у шины его нет); сообщения, которые шлют Lua, ролики и база очков `score`.
- Реакция большинства классов-действий в прогоне без игры — «чёрный ящик».

## Подробнее

- [Логика миссий и Lua](../../formats/mission-logic.md) §1, §4, §6
- [Граф логики миссий](../../logic-graph.md) §1–§2, §8, §9
- [Живая связь](../../analysis/live-link.md) §13.6–13.7
- [Нью-Йорк: устройство карты](../../analysis/queens.md) §7
- [Стенд проверки в игре](../../analysis/game-bench.md) §30
- [Визуальные скрипты](../../scripts.md) §1
