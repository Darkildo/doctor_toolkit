---
type: question
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/scripts.md §9", "docs/scripts.md §3", "docs/analysis/lua-api.md §9", "docs/analysis/game-bench.md §35"]
tags: [question, lua, messages]
updated: 2026-11-16
---
# Стоповое сообщение скрипта (`shutdown`) в игре не проверялось

Решено этапом 74. Стоповое сообщение объекта `LuaAudioScript` зовёт `CallLuaFunction(имя, "shutdown")` (`0x480D66`); то же делает деструктор (`0x480A2E`). На стенде: после `live send` стопового сообщения обработчик «On stop» выполнен и таймеры скрипта больше не срабатывают; повторное стартовое сообщение выполняет чанк заново, скрипт работает снова. Скрипт, не вызвавший `RegisterFunction`, не получает ни `startup`, ни `shutdown`: имя скрипта попадает в объект через `RegisterScriptName`.

## Следующий шаг

Не требуется.

## Связанное

- [Lua в игре](../engine/lua.md)
- [docs/scripts.md](../../scripts.md) §9
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
