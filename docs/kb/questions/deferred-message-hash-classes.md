---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 78, 79]
sources: ["docs/analysis/message-bus.md §6", "research/logic_schema.py", "data/gf2/logic_schema.toml", "docs/logic-graph.md §2", "data/gf2/class_behavior.toml"]
tags: [question, messages, simgroup, exe]
updated: 2026-11-16
---
# Классы, где сообщение было выведено по данным («12 классов хэшируют имя не в читателе»)

Ответ (этап 78): все 44 пина-сообщения в 29 классах-пакетах, которые схема держала с уверенностью `data`, прочитаны в коде. 33 подтверждены (17 `in`, 16 `out`), у 8 сторона была обратной, 3 оказались не сообщениями шины.

- «Отложенный хэш» — частный случай. `AndEventGate`, `MessageRelay`, `ForwardSender` хэшируют и подписывают имя сразу в читателе; скрипт читателей не видел этого из-за общей метки, вспомогательной функции или таблицы переходов.
- Позже читателя имя попадает в таблицу у пяти классов: `ChangeBuildingSettings`, `ChangeNPCSettings`, `ChangeDialog` (объект отдаёт имя зданию или персонажу, шлёт уже тот), `PrioritizeEvents` (подписывается менеджер), `TODSequencerAction` (подписка в конструкторе).
- Сторона изменилась: `GangsterNpcRequester` 7/9/11/13 и `WitnessCrime` 1 — слушают; `ChangeBuildingSettings` 4, `ChangeDialog` 4, `GenericCSA` 9 — шлют.
- Не сообщения: `StealMutator` 1 и класс `aeaf6570` 2 (хэш события счёта), `Item` 31 (имя предмета).
- Схема: `in` — 210 `exe`, `out` — 140 `exe`, пинов `data` не осталось.

Не доказано одно: место отправки `PlayerTeleportRelay` 2 (ручка `+0xB4` создаётся и не подписывается — по правилу схемы `out`; кандидат `0x007DC99B`). Режим `MessageRelay` с хэшем в атрибуте 2 (`0x0043C740`) не разобран.

## Что выяснено (этап 79)

- `PlayerTeleportRelay` 2 (`on_arrived`) — выход: его шлёт не реле, а автомат телепорта игрока `0x007DC3B0` в состоянии 9 (`0x0045C400`), в данных — игрок.
- `MessageRelay` с атрибутом 2: это имя группы движка (`GroupNPC`, `GroupVehicle`, `GroupDoors`… — таблица `0x0110ABA0`); `0x0043C740` находит группу среди 64 записей менеджера `*0x01223414`, `0x0043C6F0` доставляет сообщение слушателю каждого её члена (`0x00408BF0`). В файлах игры режим не используется.

Подробнее — заметки классов [`PlayerTeleportRelay`](../classes/PlayerTeleportRelay.md) и [`MessageRelay`](../classes/MessageRelay.md).

## Связанное

- [Шина сообщений](../engine/message-bus.md)
- [Класс `AndEventGate`](../classes/AndEventGate.md)
- [Класс `MessageRelay`](../classes/MessageRelay.md)
- [Класс `ForwardSender`](../classes/ForwardSender.md)
- [Разбор шины сообщений](../../analysis/message-bus.md) §6
