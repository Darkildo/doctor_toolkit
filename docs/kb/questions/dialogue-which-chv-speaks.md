---
type: question
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 83]
sources: ["docs/formats/dialogues.md §3.1", "docs/formats/dialogues.md §8"]
tags: [question, dialogues, speech]
updated: 2026-11-16
---
# Какой `chv` говорит реплику диалога

Правило игры найдено на этапе 83 по коду; редактор диалогов его ещё не применяет, на стенде оно не сверено.

## Что известно

- У говорящего четыре слота наборов речи — атрибуты 0–3 пакета [ChatterComponent](../classes/ChatterComponent.md) (читатель `0x006F38A0`).
- Событие ищется в слотах в порядке 3, 2, 1, 0 (`0x006F3B80`); говорит первый набор, в котором событие есть.
- Объект [SimpleChatterBox](../classes/SimpleChatterBox.md) может дать свой набор (атрибут 7): тогда событие сначала ищут в нём (`0x006F2070`).
- `chv` находится по GUID ресурса (`0x006F4680`).

## Следующий шаг

Построить в индексе переход «диалог → NPC (атрибут с путём потока) → `ChatterComponent` → `chv`» и показывать в редакторе реплику по правилу слотов; сверить со звуком на стенде (диалог с Майклом на террасе, запись звука).

## Связанное

- [Диалоги F2F и речь](../systems/dialogues.md)
- [Класс `ChatterComponent`](../classes/ChatterComponent.md)
- [Тип ресурса `chv`](../resources/chv.md)
- [docs/formats/dialogues.md](../../formats/dialogues.md) §3.1, §8
