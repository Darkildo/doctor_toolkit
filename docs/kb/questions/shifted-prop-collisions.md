---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/havok.md §5", "docs/formats/havok.md §1.3", "docs/levels.md §11.7"]
tags: [question, physics, havok]
updated: 2026-11-16
---
# Смещённые коллизии реквизита заведений и террейна

Коллизии реквизита заведений и террейна в данных смещены относительно моделей; в редакторе они стоят там, где их ставит metamodel. Откуда берётся сдвиг в игре, не известно.

## Следующий шаг

Найти в exe, какой трансформ применяется к телу коллизии при создании объекта; сравнить с положением тел в живой игре.

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Физика: Havok и коллизии](../engine/physics.md)
- [Тип ресурса `hkx`](../resources/hkx.md)
- [docs/formats/havok.md](../../formats/havok.md) §1.3, §5
