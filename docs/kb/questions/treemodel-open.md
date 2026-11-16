---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/treemodel.md §6"]
tags: [question, treemodel, tooling]
updated: 2026-11-16
---
# `treemodel`: последнее поле вершин листьев и сборка новых деревьев

Не известно значение `k / 255` последнего поля вершин листьев (возможно, затенение кроны для другого прохода). Правки деревьев в редакторе нет: ресурс читается и пишется обратно побайтно, новых деревьев инструмент не собирает.

## Следующий шаг

Сверить поле с шейдером листьев; реализовать сборку `treemodel` из частей.

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Тип ресурса `treemodel`](../resources/treemodel.md)
- [docs/formats/treemodel.md](../../formats/treemodel.md) §6
