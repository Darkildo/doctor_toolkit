---
type: question
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/scripts.md §9", "docs/analysis/lua-api.md §10", "crates/doctor-toolkit/src/resource/lua/vm.rs", "docs/analysis/game-bench.md §35"]
tags: [question, lua, tooling]
updated: 2026-11-16
---
# Сопрограммы (`coroutine`) в ВМ тулкита не поддержаны

Решено этапом 74. ВМ тулкита поддерживает `coroutine.create`, `wrap`, `resume`, `yield`, `status`: у сопрограммы свой стек значений, уступить можно только сквозь кадры Lua (как в Lua 5.0 — «attempt to yield across metamethod/C-call boundary»). Набор проверок выполнен и в игре (`live lua`), и в ВМ: результаты и тексты ошибок совпали, кроме имени функции в одной ошибке аргумента. С отдельной сборкой Lua 5.0.3 ВМ не сверялась.

## Следующий шаг

Не требуется; при появлении расхождений — сверить с Lua 5.0.3 на тестах её дистрибутива.

## Связанное

- [Lua в игре](../engine/lua.md)
- [docs/scripts.md](../../scripts.md) §9
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
