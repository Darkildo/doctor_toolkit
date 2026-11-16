---
type: question
status: unknown
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 97]
sources: ["docs/formats/cutscenes.md §8", "docs/formats/cutscenes.md §9.1"]
tags: [question, cutscenes, simgroup]
updated: 2026-11-16
---
# Какой атрибут задаёт флаг привязки `TrinityGameSequence` (+0x118, бит 2)

Секвенсор применяет привязку ролика по флагу объекта `TrinityGameSequence` (+0x118, бит 2). Какой атрибут SimGroup ставит этот флаг, не известно. Не известен и смысл атрибута 7 (GUID, не ресурс).

## Следующий шаг

Разобрать читатель атрибутов `TrinityGameSequence` в Ghidra.

Запланировано: [Этап 97: Ролики](../research/stage-97.md).

## Связанное

- [Класс `TrinityGameSequence`](../classes/TrinityGameSequence.md)
- [Ролики](../systems/cutscenes.md)
- [Ролики: секвенсор Trinity](../exe/cutscenes-sequencer.md)
- [docs/formats/cutscenes.md](../../formats/cutscenes.md) §8, §9.1
