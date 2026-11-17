---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 27, 79]
sources: ["docs/formats/mission-logic.md §6", "docs/logic-graph.md §9", "data/gf2/class_behavior.toml", "docs/analysis/class-behavior.md §4"]
tags: [question, simgroup, logic]
updated: 2026-11-16
---
# Смысл атрибутов 1 и 2 `MultiManager`

По коду: атрибут 1 — число срабатываний (−1 у 9 015 объектов), атрибут 2 — флаги. Названия этапа 27 помечены `guess`.

## Что выяснено (этап 79)

Читатель `0x0047A320` и обработчик `0x0047A1C0` прочитаны до конца.

- Атрибут 1 (`+0x60`) — остаток срабатываний: −1 — без конца; положительный уменьшается на каждом срабатывании; при 0 объект не срабатывает, а дойдя до 0, выключает слушатель (`0x00449E90`). В игре: −1 у 9 015 объектов, 1 у 623, 9 у одного.
- Атрибут 2 (`+0x5E`, `u16`) — флаги: бит 0 (1) — одна пара за срабатывание, по кругу; бит 1 (2) — помнить остаток в хранилище состояния (`iMsgStoreEntity` / `iMsgRestoreEntity`, запись `0x5E1F74A6`, при нуле — метка `0x137726B7`); бит 2 (4) — отложенные отправки ставятся в очередь с флагом 4 (переживают сброс очереди). В игре: 4 у 1 469 объектов, 2 у четырёх, 1 у двух, 3 у одного.

На стенде: см. [опыт](../experiments/class-state-in-game.md).

Сделано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Класс `MultiManager`](../classes/MultiManager.md)
- [Сюжет и миссии](../systems/story.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §4, §6
