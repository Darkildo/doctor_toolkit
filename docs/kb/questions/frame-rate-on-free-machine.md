---
type: question
status: unknown
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 58, 101]
sources: ["docs/analysis/performance.md §9"]
tags: [question, performance, bench]
updated: 2026-11-16
---
# Абсолютная частота кадров на свободной машине и на Windows

Замеры частоты сделаны на стенде, который делился с другими прогонами, с выводом через Xvfb без DRI3. Абсолютная частота на свободной машине и на Windows не измерена. Не проверены ролики (угол и обрезка при 21:9), езда и стрельба на разной частоте.

## Следующий шаг

Повторить замеры на свободной машине и на Windows; прогнать ролик при 21:9.

Запланировано: [Этап 101: Меньше вызовов отрисовки](../research/stage-101.md).

## Связанное

- [Рендер](../engine/render.md)
- [Темп игры и частота кадров: замеры на стенде](../experiments/game-pace-and-frame-rate.md)
- [docs/analysis/performance.md](../../analysis/performance.md) §9
