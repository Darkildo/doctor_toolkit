---
type: question
status: hypothesis
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/formats/mission-logic.md §6", "docs/formats/mission-logic.md §3"]
tags: [question, lua, exe]
updated: 2026-11-16
---
# Используется ли состояние `+0x1C` менеджера Lua

Игра держит два состояния Lua: `+0x1C` (создаётся при старте приложения, `0x43E5D0`) и `+0x18`; функции скриптов работают с `+0x18`. Гипотеза: `+0x1C` в релизе не используется.

## Следующий шаг

Найти в Ghidra все обращения к `+0x1C` и проверить, загружается ли в это состояние хоть один чанк.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Lua 5.0 в exe: состояние, загрузка и вызов](../exe/lua-runtime.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §3, §6
