---
type: research
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [78]
sources: ["docs/analysis/message-bus.md", "docs/analysis/game-bench.md §36", "research/bus_sites.py", "research/message_senders.py", "research/logic_schema.py", "data/gf2/bus_sites.tsv", "data/gf2/message_senders.tsv", "data/gf2/messages.toml"]
tags: [research, messages]
updated: 2026-11-16
---
# Этап 78: Шина сообщений

- Состояние: сделан (2026-11-16) по коду exe, корпусу и стенду.
- Зависит от: [этап 73](stage-73.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- **Шина**: три пути доставки — рассылка `0x00408A00`, [доставка одному слушателю `0x00408BF0` и очередь задержки `0x00440590`](../engine/message-delivery.md); порядок по приоритетам, повторный вход, отложенные подписки. Исправлены адреса прежних документов (`0x00408B70`, `0x004404F0`, `0x010F6F2C`, `0x0110064C`).
- **Данные сообщений**: [указатель на объект `0x00E31E2C`, пересылка, 18 структур движка](../engine/message-data.md); `data` заполнено у 309 имён движка и 30 785 сообщений файлов игры.
- **Отправители**: `research/bus_sites.py` → `data/gf2/bus_sites.tsv` — отправитель в коде у 320 имён (было 9), слушатель — у 262; 74 новых имени в списке движка (645). Lua, ролики и `score` — `data/gf2/message_senders.tsv` (202 сообщения).
- **Схема пинов**: все 44 пина `data` прочитаны в коде, 8 сменили сторону, 3 — не сообщения.
- **`ExecuteVFX` 2–5**: обычные имена шины, их никто не шлёт.
- **Живая связь**: протокол 8 — место вызова, причина, отправитель, объект в данных, доставки одному слушателю; [опыт на стенде](../experiments/message-bus-senders.md).

## Открытые вопросы после этапа

- [Данные: чей объект у части классов, `data` в Lua](../questions/lua-message-data.md)
- [Отправители в коде у остальных имён движка](../questions/engine-message-senders.md)
- [Отправитель из обновления объекта и из очереди](../questions/message-sender-unknown.md)
- [Порядок слушателей](../questions/message-listener-order.md)
- [Точки останова для доставки одному слушателю](../questions/direct-delivery-breakpoints.md)
- [Слушатели Lua и «немые» сообщения](../questions/non-simgroup-message-senders.md)
- [`PlayerTeleportRelay` 2, `MessageRelay` с хэшем](../questions/deferred-message-hash-classes.md)
- [Кто посылает `ExecuteVFX` 2–5](../questions/executevfx-show-hide-senders.md) — закрыт

## Правила этапа

Этап обязан обновить базу знаний: вопросы получают новый статус, появляются новые заметки, у каждого утверждения есть источник. См. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
