---
type: question
status: hypothesis
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 78]
sources: ["docs/logic-graph.md §8"]
tags: [question, messages, vfx]
updated: 2026-11-16
---
# Кто посылает `ExecuteVFX` 2–5 (`show`, `hide`, `shadow_on`, `shadow_off`)

Атрибуты 2–5 `ExecuteVFX` (`show`, `hide`, `shadow_on`, `shadow_off`) подписаны на сообщения, но никто их не посылает. Вероятно, их шлёт сам объект или движок адресно.

## Следующий шаг

Найти в Ghidra адресную отправку (`0x00408B70`) этих сообщений и её вызывающих.

Запланировано: [Этап 78: Шина сообщений](../research/stage-78.md).

## Связанное

- [Класс `ExecuteVFX`](../classes/ExecuteVFX.md)
- [Шина сообщений](../engine/message-bus.md)
- [docs/logic-graph.md](../../logic-graph.md) §8
