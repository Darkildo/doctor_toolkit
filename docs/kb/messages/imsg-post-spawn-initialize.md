---
type: message
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 27]
sources: ["docs/logic-graph.md §9.2", "data/gf2/logic_schema.toml packet.TriggerAuto"]
tags: [messages, engine, simgroup]
updated: 2026-11-16
---
# Сообщение `iMsgPostSpawnInitialize`

Сообщение движка: он шлёт его объектам загруженной SimGroup (переменная сообщения `0x0110064C`, имя задаёт `0x00445530`).

- `TriggerAuto` (обработчик `0x00477EF0`, подписка `0x00477DA0`) срабатывает по нему один раз и шлёт сообщение атрибута 0 (`send`) с задержкой из атрибута 1 (`delay`).
- `VariableWatcher` начинает следить за переменной после `activate` или по этому сообщению, если у атрибута 7 стоит флаг 1.

Так запускается логика уровня без внешнего события; прогон логики в тулките начинает с того же сообщения.

## Связанное

- [Шина сообщений](../engine/message-bus.md)
- [Класс `TriggerAuto`](../classes/TriggerAuto.md)
- [Класс `VariableWatcher`](../classes/VariableWatcher.md)
- [Шина сообщений: имена, подписка, отправка](../exe/message-bus.md)
- [Сообщения движка: полный список](engine-messages.md)
- [docs/logic-graph.md](../../logic-graph.md) §9.2
