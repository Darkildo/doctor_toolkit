---
type: question
status: unknown
confidence: exe
editions: ["1.0.789.0"]
stages: [78]
sources: ["docs/analysis/message-bus.md §3.1", "docs/analysis/live-link.md §11.3", "crates/doctor-toolkit-live-asi/src/journal.rs"]
tags: [question, messages, live-link, breakpoints]
updated: 2026-11-16
---
# Точки останова и счётчики не видят доставку одному слушателю

Игра доставляет сообщения не только рассылкой `0x00408A00`: 127 мест зовут `0x00408BF0` и её обёртки, а очередь задержки зовёт обработчик адресата сама. Перехват модуля связи стоит на входе `0x00408A00`. С протокола 8 журнал такие доставки записывает (переходники `0x00408C31`, `0x00440819`), но точка останова по сообщению (`live break --message`) на них не срабатывает, а счётчики `MessageCounts` их не считают. Например, `iMsgPostSpawnInitialize` и `iMsgTrigger` идут только так.

## Следующий шаг

В `on_direct` (`crates/doctor-toolkit-live-asi/src/journal.rs`) звать `debug::before_message` так же, как в `on_bus`, и считать отброшенные пределом частоты в `COUNTS`; проверить на стенде точку останова на `iMsgTrigger` при входе игрока в объём.

## Связанное

- [Доставка одному слушателю и очередь задержки](../engine/message-delivery.md)
- [Модуль в игре `doctor_live` (живая связь)](../engine/live-module.md)
