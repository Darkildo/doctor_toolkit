---
type: question
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 28, 74]
sources: ["docs/formats/mission-logic.md §6", "docs/analysis/lua-api.md §6", "docs/analysis/game-bench.md §35"]
tags: [question, lua, timers]
updated: 2026-11-16
---
# Как таймеры вызывают функцию Lua по имени

Решено этапом 74. Такт таймеров — обработчик `iMsgRunningTick` (`0x467690`) → `0x467440`. Сработавший таймер перезаряжается или останавливается, затем имя функции ищется в глобальных при каждом срабатывании; если там функция, она зовётся через `luaD_pcall` (`0x467645`) с одним аргументом — номером таймера. Не функция — пропуск без ошибки. Часы — игровые `0x01205224` или реальные (третий аргумент `CreateTimer`). На стенде: обработчик получил номер, одноразовый таймер сработал один раз, таймер с несуществующей функцией игру не роняет.

## Следующий шаг

Не требуется.

## Связанное

- [Lua в игре](../engine/lua.md)
- [Lua 5.0 в exe: состояние, загрузка и вызов](../exe/lua-runtime.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §6
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
