---
type: question
status: hypothesis
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/geometry.md §8", "docs/formats/geometry.md §7.2"]
tags: [question, geometry]
updated: 2026-11-16
---
# Как игра выбирает размер ячеек сетки треугольников и использует ли сетку

Меш хранит сетку треугольников (0x8C). Как игра выбирает размер ячеек и использует ли она сетку, не известно; вероятно, сетка нужна для попаданий и декалей.

## Следующий шаг

Найти потребителя сетки в exe.

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Тип ресурса `mesh`](../resources/mesh.md)
- [docs/formats/geometry.md](../../formats/geometry.md) §7.2, §8
