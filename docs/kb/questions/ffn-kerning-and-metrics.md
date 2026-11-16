---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 94]
sources: ["docs/formats/resource-types.md §11"]
tags: [question, ui, fonts]
updated: 2026-11-16
---
# Шрифты `ffn`: раскладка кернинга и метрики

В `ffn` не установлен смысл трёх u32 метрик шрифта (у impact: 43, 50, 41) и не разобрана раскладка кернинга (по 3 байта на запись).

## Следующий шаг

Найти чтение кернинга в коде вывода текста.

Запланировано: [Этап 94: Интерфейс](../research/stage-94.md).

## Связанное

- [Тип ресурса `ffn`](../resources/ffn.md)
- [Интерфейс: APX (Apt), шрифты, строки](../engine/ui-apt.md)
- [docs/formats/resource-types.md](../../formats/resource-types.md) §11
