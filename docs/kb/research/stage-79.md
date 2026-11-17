---
type: research
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [79]
sources: ["docs/analysis/class-behavior.md", "data/gf2/class_behavior.toml", "docs/analysis/game-bench.md §41", "research/class_behavior.py", "research/class_usage.py", "research/bench_class_state.py"]
tags: [research, simgroup, classes, logic]
updated: 2026-11-16
---
# Этап 79: Поведение классов SimGroup, волна 1

- Состояние: сделан (2026-11-16) по коду exe и корпусу; на стенде — один короткий сеанс.
- Зависит от: [этап 73](stage-73.md), [этап 78](stage-78.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- **Код 45 классов и 5 общих пакетов прочитан**: 23 блока полностью, 27 частично; 142 реакции обработчиков, 462 атрибута с именем, видом значения и полем объекта — `data/gf2/class_behavior.toml`; в заметках классов — разделы «Поведение» и «Атрибуты по коду». Обзор и общие правила — [Поведение классов SimGroup](../../analysis/class-behavior.md).
- **Вопросы этапа**: [`MultiManager` 1–2](../questions/multimanager-attrs-1-2.md), [флаги `TriggerAuto`](../questions/triggerauto-flags.md), [`CSystemCommands` 2–3](../questions/csystemcommands-attrs-2-3.md), [байт 0x1D объекта](../questions/simgroup-object-byte-1d.md), [отложенный хэш](../questions/deferred-message-hash-classes.md) — закрыты; [`TriggerBase` 6, 8–10](../questions/triggerbase-attrs-6-8-10.md), [виды значений](../questions/simgroup-value-kinds.md), [`Bool` и `Money`](../questions/attr-bool-and-money-types.md), [имена](../questions/attribute-and-class-names.md), [обработчик `EnterExitTrigger`](../questions/missiontimer-enterexittrigger-handlers.md) — частично.
- **Схема и типы**: имена и направления сообщений из кода — в `logic_schema.toml` (226 атрибутов добавлены, 72 названы, 9 переименованы, 25 пинов-сообщений новые или сменили сторону); каталог сообщений вырос до 42 087 имён, у 96 «немых» нашёлся отправитель; в `attr_types.toml` 4 догадки `bool` подтверждены, 2 догадки-ссылки сняты, 13 типов добавлены по коду.
- **Прогон логики без игры** исправлен по коду и закреплён тестами: начальное состояние и счётчик триггеров, `TriggerBase` как датчик касания, веса `TriggerRandom`, порядок проверок `AndEventGate`, предел `VariableWatcher`, `VariableSwitch`, `MessageRelay`.
- **Стенд**: [состояние объектов в игре](../experiments/class-state-in-game.md) — таблицы методов, удаление отработавших объектов, одноразовый `MultiManager`.

## Открытые вопросы после этапа

- [Как часто физика сообщает триггеру о касании](../questions/trigger-touch-frequency.md)
- [Хранилище состояния: что попадает в сохранение](../questions/state-store-records.md)
- [Порядок слушателей](../questions/message-listener-order.md) — добавлен приоритет `TriggerAuto`
- [Виды значений остальных классов](../questions/simgroup-value-kinds.md), [имена](../questions/attribute-and-class-names.md) — следующие волны

## Правила этапа

Этап обязан обновить базу знаний: вопросы получают новый статус, появляются новые заметки, у каждого утверждения есть источник. См. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
