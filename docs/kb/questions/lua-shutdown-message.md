---
type: question
status: unknown
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/scripts.md §9", "docs/scripts.md §3"]
tags: [question, lua, messages]
updated: 2026-11-16
---
# Стоповое сообщение скрипта (`shutdown`) в игре не проверялось

Узел «On stop» графа скриптов срабатывает при стоповом сообщении (`shutdown`); `start.stop_message` вызывает `shutdown`. В игре это не проверялось.

## Следующий шаг

На стенде послать стоповое сообщение загруженному скрипту (`live send`) и проверить по трассировке, что обработчик вызван и таймеры остановлены.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [docs/scripts.md](../../scripts.md) §9
