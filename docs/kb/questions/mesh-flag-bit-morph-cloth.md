---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/geometry.md §8"]
tags: [question, geometry, mesh]
updated: 2026-11-16
---
# Бит `0x1` флагов записи, морф-каналы `0x3A` и биты одежды материала

После этапа 39 в формате мешей остаются неназванными: бит `0x1` флагов записи отрисовки, морф-каналы `0x3A` заголовка, биты одежды материала.

## Следующий шаг

Найти чтение этих полей в загрузчике мешей (Ghidra); сравнить меши с разными значениями по корпусу.

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Тип ресурса `mesh`](../resources/mesh.md)
- [Тип ресурса `material`](../resources/material.md)
- [Модели: загрузчик мешей и буферы](../exe/mesh-buffers.md)
- [docs/formats/geometry.md](../../formats/geometry.md) §8
