---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 84]
sources: ["docs/analysis/gf2-pc-verification.md §6", "docs/formats/stream.md §6.1"]
tags: [question, stream]
updated: 2026-11-16
---
# Смысл u16 со значением 3 в заголовке потока

В заголовке потока по смещению +0x0A лежит u16, всегда равный 3. Смысл не известен.

Этап 84 поле не разобрал: разбор блоков `0x004D0DB0` сравнивает только тег `SHOC`, сравнения магии файла `3slo` константой в коде не найдено — читатель 12-байтового заголовка не найден.

## Следующий шаг

Найти чтение поля в загрузчике потоков (см. заметки exe о менеджере потоков).

Запланировано: [Этап 84: Загрузка зон](../research/stage-84.md).

## Связанное

- [Потоки `.str` и их загрузка](../engine/stream-loading.md)
- [Менеджер потоков: запрос, запись, выгрузка](../exe/stream-manager.md)
- [docs/formats/stream.md](../../formats/stream.md) §6.1
