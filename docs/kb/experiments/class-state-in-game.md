---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [79]
sources: ["docs/analysis/game-bench.md §41", "research/bench_class_state.py", "data/gf2/class_behavior.toml"]
tags: [experiment, simgroup, logic, live]
updated: 2026-11-16
---
# Опыт: состояние объектов логических классов в запущенной игре

Проверялось, совпадает ли прочитанное в коде классов с тем, что видно в игре через живую связь (Гавана, 77-я секунда игры, 61 загруженный поток).

## Как

`live peek` таблиц методов; `live objects --all --json` и `live streams`, затем `research/bench_class_state.py report`: объекты классов в SimGroup загруженных потоков против объектов, которые есть в игре; `live send … --to GUID` объекту `MultiManager` со счётчиком 1.

## Результат

- Таблицы методов в памяти совпадают с прочитанными: обработчик `EnterExitTrigger` — `0x00900E20` (слот 1 таблицы `0x00D817D8`), `MultiManager` — `0x0047A1C0`, `TriggerAuto` — `0x00477EF0`, `TriggerBox` — `0x004C1260`.
- Из 55 объектов `TriggerAuto` загруженных потоков в игре нет ни одного: объект, который «отработал» (`0x004088C0`: слушатель выключен, подписка на `iMsgDeleteEntity`), удаляется.
- `MultiManager` со счётчиком 1 после одного сообщения исчез из списка объектов; второе сообщение ему уже не доставляется.
- `MultiManager` со счётчиком −1 (357 из 358), `VariableSwitch` (35 из 36), `EnterExitTrigger` (8 из 8) на месте и слушают.

## Что из этого следует

Выключение слушателя по коду — это удаление объекта в игре: «одноразовые» объекты логики после срабатывания нельзя разбудить сообщением, их нет. Прогон логики без игры ведёт себя так же (объект больше не реагирует).

Не проверено на стенде: сама отправка сообщений `MultiManager` в этом опыте, флаги сохранения состояния, триггеры-объёмы.

## Связанное

- [Поведение классов SimGroup](../../analysis/class-behavior.md)
- [Класс `TriggerAuto`](../classes/TriggerAuto.md), [класс `MultiManager`](../classes/MultiManager.md)
- [Вопрос: атрибуты 1 и 2 `MultiManager`](../questions/multimanager-attrs-1-2.md)
