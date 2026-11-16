---
type: question
status: hypothesis
confidence: data
editions: ["1.0.789.0"]
stages: [72, 98]
sources: ["docs/formats/resource-types.md §8"]
tags: [question, resource-type, format]
updated: 2026-11-16
---
# Формат ресурса `score` не разобран

Тип `score`: гипотеза: база событий счёта и достижений, подписанная на шину сообщений (версия 10, около 1200 имён сообщений). Разбора в тулките нет.

## Следующий шаг

Выгрузить ресурсы (`doctor-toolkit dump <игра> --type score`), описать раскладку и найти загрузчик в exe.

Запланировано: [Этап 98: Неопознанные типы ресурсов](../research/stage-98.md).

## Связанное

- [Тип ресурса `score`](../resources/score.md)
- [docs/formats/resource-types.md](../../formats/resource-types.md) §8
