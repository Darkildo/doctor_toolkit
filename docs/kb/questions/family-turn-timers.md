---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 86]
sources: ["docs/analysis/tunables.md §3"]
tags: [question, families]
updated: 2026-11-16
---
# Какой атрибут `Family` 18–21 какой таймер хода

Атрибуты 18–21 `Family`: `corleone` 720/720, соперники 240–360 и 630–1310 секунд. Таблица Cheat Engine показывает у семьи таймеры «Minimum/Maximum time before next turn», но какой атрибут какой таймер, не установлено.

## Следующий шаг

Разобрать читатель `Family` в Ghidra; сверить значения с памятью живой игры (`live`).

Запланировано: [Этап 86: Семьи и война](../research/stage-86.md).

## Связанное

- [Семьи и война семей](../systems/families.md)
- [Класс `Family`](../classes/Family.md)
- [docs/analysis/tunables.md](../../analysis/tunables.md) §3
