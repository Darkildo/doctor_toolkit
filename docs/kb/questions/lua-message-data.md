---
type: question
status: unknown
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 73, 74, 78]
sources: ["docs/scripts.md §9", "docs/logic-graph.md §1", "data/gf2/messages.toml"]
tags: [question, lua, messages]
updated: 2026-11-16
---
# Что лежит в данных сообщения (`data`) и как читать их из Lua

Обработчик сообщения в Lua получает `data` — userdata. Что внутри и как это читать из скрипта, не известно. `SendMessage` с данными и то, что получатель видит в `data`, в игре не проверялись. В коде движка у сообщений есть тип данных (`"BasePtrMsgData*"` при подписке `0x00473EC0`).

В типах этапа 73 у каждого сообщения есть поле `data`; оно равно `unknown` у всех 42 245 известных сообщений (571 имя из exe и 41 674 имени из файлов игры) — заполнять его будет это исследование через `data/gf2/messages.toml`.

## Следующий шаг

Разобрать в Ghidra типы данных сообщений (`BasePtrMsgData*` и другие) и функции Lua, которые отдают `data` скрипту; проверить на стенде `SendMessage` с данными.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md), [Этап 78: Шина сообщений](../research/stage-78.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Шина сообщений](../engine/message-bus.md)
- [Шина сообщений: имена, подписка, отправка](../exe/message-bus.md)
- [docs/scripts.md](../../scripts.md) §9
