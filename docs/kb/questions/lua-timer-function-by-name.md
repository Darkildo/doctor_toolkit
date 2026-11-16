---
type: question
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 28, 74]
sources: ["docs/formats/mission-logic.md §6"]
tags: [question, lua, timers]
updated: 2026-11-16
---
# Как таймеры вызывают функцию Lua по имени

По поведению таймер вызывает глобальную функцию Lua с этим именем — проверено этапом 28. Путь вызова в коде движка не разобран.

## Следующий шаг

Разобрать в Ghidra обработчик таймера до вызова Lua (поиск глобальной функции по имени) и записать адреса в заметку exe.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Lua 5.0 в exe: состояние, загрузка и вызов](../exe/lua-runtime.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §6
