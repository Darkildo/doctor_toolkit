---
type: question
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/scripts.md §9"]
tags: [question, lua, tooling]
updated: 2026-11-16
---
# Сопрограммы (`coroutine`) в ВМ тулкита не поддержаны

ВМ Lua тулкита (прогон скриптов без игры) не поддерживает сопрограммы; генератор скриптов их не использует. Скрипты игры с `coroutine` в ВМ тулкита не выполнятся.

## Следующий шаг

Добавить сопрограммы в ВМ тулкита, когда появится скрипт, которому они нужны.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [docs/scripts.md](../../scripts.md) §9
