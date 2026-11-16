---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 78]
sources: ["docs/formats/mission-logic.md §6", "docs/logic-graph.md §2"]
tags: [question, messages, simgroup, exe]
updated: 2026-11-16
---
# 12 классов хэшируют имя сообщения не в читателе атрибутов

У 12 классов (`AndEventGate` 0–3, `MessageRelay` 0, `ForwardSender` 0, …) строка сообщения хэшируется и попадает на шину не в читателе атрибутов; роль этих атрибутов установлена по данным, а не по коду.

## Следующий шаг

Найти в Ghidra, где эти классы хэшируют строку (отложенный хэш), и перевести пины из `data` в `exe` в `data/gf2/logic_schema.toml`.

Запланировано: [Этап 78: Шина сообщений](../research/stage-78.md).

## Связанное

- [Шина сообщений](../engine/message-bus.md)
- [Класс `AndEventGate`](../classes/AndEventGate.md)
- [Класс `MessageRelay`](../classes/MessageRelay.md)
- [Класс `ForwardSender`](../classes/ForwardSender.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §6
