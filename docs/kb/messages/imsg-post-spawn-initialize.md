---
type: message
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 27, 78]
sources: ["docs/logic-graph.md §9.2", "data/gf2/logic_schema.toml packet.TriggerAuto", "data/gf2/bus_sites.tsv", "docs/analysis/message-bus.md §3.1"]
tags: [messages, engine, simgroup]
updated: 2026-11-16
---
# Сообщение `iMsgPostSpawnInitialize`

Сообщение движка: он шлёт его объектам загруженной SimGroup (переменная сообщения `0x0120E93C`, имя задаёт `0x004455D0`; доставка каждому слушателю отдельно — `0x00408BF0` из `0x00446470` и `0x00855D40`; прежние адреса `0x0110064C` и `0x00445530` неверны: ссылок на них в коде нет).

- `TriggerAuto` (обработчик `0x00477EF0`, подписка — в конструкторе `0x00477DB0`, приоритет `0x7FFF`) срабатывает по нему один раз и шлёт сообщение атрибута 0 (`send`) с задержкой из атрибута 1 (`delay`).
- `VariableWatcher` начинает следить за переменной после `activate` или по этому сообщению, если у атрибута 7 стоит флаг 1.

Так запускается логика уровня без внешнего события; прогон логики в тулките начинает с того же сообщения.

## Связанное

- [Шина сообщений](../engine/message-bus.md)
- [Класс `TriggerAuto`](../classes/TriggerAuto.md)
- [Класс `VariableWatcher`](../classes/VariableWatcher.md)
- [Шина сообщений: имена, подписка, отправка](../exe/message-bus.md)
- [Сообщения движка: полный список](engine-messages.md)
- [docs/logic-graph.md](../../logic-graph.md) §9.2
