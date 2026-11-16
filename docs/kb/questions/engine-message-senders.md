---
type: question
status: unknown
confidence: exe
editions: ["1.0.789.0"]
stages: [73, 78]
sources: ["data/gf2/engine_messages.txt", "data/gf2/game_messages.tsv", "docs/logic-graph.md §2.1"]
tags: [question, messages, exe]
updated: 2026-11-16
---
# Кто в коде движка шлёт сообщения группы `Engine`

Из 571 имени сообщения, названного в `godfather2.exe`, объекты SimGroup используют 198: 150 только слушают (значит, шлёт движок), 34 только шлют (слушает движок), 14 и шлют, и слушают. Остальные 373 живут только в коде. Какая функция шлёт каждое сообщение, при каком событии и с какими данными, известно для единиц (`iMsgPostSpawnInitialize`, сообщения кадра — см. [сообщения файлов игры](../messages/game-messages.md)); в типах у остальных стоит только «имя названо в exe».

## Следующий шаг

В Ghidra для каждой переменной сообщения (ручки, созданной `0x00408240`) собрать места вызова отправки `0x00408A00` и подписки `0x004089B0`; записать отправителя, условие и данные в `data/gf2/messages.toml` (поля `summary`, `sent_by`, `data`).

Запланировано: [Этап 78: Шина сообщений](../research/stage-78.md).

## Связанное

- [Сообщения движка: полный список](../messages/engine-messages.md)
- [Шина сообщений](../engine/message-bus.md)
- [Шина сообщений: имена, подписка, отправка](../exe/message-bus.md)
- [Данные сообщения](lua-message-data.md)
