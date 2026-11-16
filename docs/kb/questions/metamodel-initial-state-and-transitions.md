---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/metamodel.md §5", "docs/levels.md §10"]
tags: [question, metamodel, states]
updated: 2026-11-16
---
# Как движок выбирает начальное состояние metamodel и применяет переходы

Не известно, как движок выбирает начальное состояние metamodel и применяет переходы; смысл операторов условий 4–6 не установлен. Редактор уровней берёт первое состояние типа.

## Следующий шаг

Разобрать в Ghidra код состояний и переходов metamodel (операторы 4–6); проверить на `Destructible` с состояниями по здоровью.

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Тип ресурса `metamodel`](../resources/metamodel.md)
- [`Destructible` показывает «обломки» после скачка на метры за кадр](destructible-debris-after-jump.md)
- [docs/formats/metamodel.md](../../formats/metamodel.md) §5
- [docs/levels.md](../../levels.md) §10
