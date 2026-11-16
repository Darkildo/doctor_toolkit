---
type: question
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/metamodel.md §5"]
tags: [question, metamodel, tooling]
updated: 2026-11-16
---
# Полная запись metamodel (`encode`) не реализована

Тулкит читает metamodel полностью, а правит значения заплатками; полной записи (`encode`) нет, поэтому новые состояния, переходы и части добавить нельзя.

## Следующий шаг

Реализовать полную запись metamodel и сверить побайтным round-trip на всех 12 315 ресурсах.

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Тип ресурса `metamodel`](../resources/metamodel.md)
- [docs/formats/metamodel.md](../../formats/metamodel.md) §5, §6
