---
type: question
status: unknown
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 58, 100]
sources: ["docs/analysis/performance.md §9"]
tags: [question, memory, laa]
updated: 2026-11-16
---
# Указатели выше 2 ГБ в коде игры с LAA

С флагом Large Address Aware память игры на стенде не уходила выше 2 ГБ, поэтому не проверено, переживает ли код игры указатели выше 2 ГБ.

## Следующий шаг

Занять нижние 2 ГБ адресного пространства и прогнать игру на стенде; искать знаковые сравнения указателей.

Запланировано: [Этап 100: Подмена ресурсов в памяти](../research/stage-100.md).

## Связанное

- [Память](../engine/memory.md)
- [Память и Large Address Aware: опыты на стенде](../experiments/memory-and-laa.md)
- [docs/analysis/performance.md](../../analysis/performance.md) §9
