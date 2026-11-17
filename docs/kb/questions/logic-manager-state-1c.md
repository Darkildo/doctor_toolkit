---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/formats/mission-logic.md §6", "docs/formats/mission-logic.md §3", "docs/analysis/lua-api.md §8"]
tags: [question, lua, exe]
updated: 2026-11-16
---
# Используется ли состояние `+0x1C` менеджера Lua

Решено этапом 74 по коду. В состояние `+0x1C` регистрируются те же функции, что и в `+0x18` (`0x43E5D0` обходит список связок и зовёт `Register` каждой; `EclipseLog` на `0x43E5EA`), в нём выполняются встроенные тексты Lua, его чистит сборщик мусора (`0x43E900`). Но чанки скриптов в него не загружаются: единственная обёртка `0x4CB170` не имеет ни вызовов, ни ссылок, а оба настоящих вызова `0x4CB0C0` берут `+0x18`. Оговорка: проверены обращения рядом с чтениями `*0x012233F4`; доступ через другой указатель не исключён.

## Следующий шаг

Не требуется.

## Связанное

- [Lua в игре](../engine/lua.md)
- [Lua 5.0 в exe: состояние, загрузка и вызов](../exe/lua-runtime.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §3, §6
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
