---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 84]
sources: ["docs/analysis/gf2-pc-verification.md §6", "docs/analysis/new-resources.md §3"]
tags: [question, stream, guid]
updated: 2026-11-16
---
# GUID потоков кадров роликов и банков диалогов не найдены

GUID потоков берутся из `wgi` и из объектов и атрибутов SimGroup; для нового потока GUID выбирается свободный. Без найденного GUID остаются кадры роликов и банки диалогов.

Этап 84 уточнил источник `wgi`: это ячейки зон и таблица потоков уровня ([формат](../../formats/wgi.md)); кадров роликов и банков диалогов в ней нет, вопрос остаётся.

## Следующий шаг

Найти в exe, откуда берётся GUID при запросе потока ролика и банка диалога.

Запланировано: [Этап 84: Загрузка зон](../research/stage-84.md).

## Связанное

- [Потоки `.str` и их загрузка](../engine/stream-loading.md)
- [Тип ресурса `wgi`](../resources/wgi.md)
- [docs/analysis/new-resources.md](../../analysis/new-resources.md) §3
