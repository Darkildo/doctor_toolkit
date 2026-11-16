---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 85]
sources: ["docs/analysis/tunables.md §3"]
tags: [question, economy]
updated: 2026-11-16
---
# Где хранятся доходы предприятий и рэкета, цены улучшений, взятки

В SimGroup их нет: `Building` (113 объектов) хранит только id заведения, город, события и строки интерфейса; `JobReward` — иконки и события; `DynamicJob`, `HitMission` — события. Цены улучшений `UpgradeData` не хранит (атрибут 4 — номер в списке). Вероятно, это в Lua или в коде игры.

## Следующий шаг

Искать в exe и Lua таблицы доходов и цен; разобрать `bricknmortarres` и `trb` (параметры заведений).

Запланировано: [Этап 85: Экономика](../research/stage-85.md).

## Связанное

- [Экономика: деньги, заведения, награды](../systems/economy.md)
- [Класс `Building`](../classes/Building.md)
- [Класс `JobReward`](../classes/JobReward.md)
- [Тип ресурса `trb`](../resources/trb.md)
- [Тип ресурса `bricknmortarres`](../resources/bricknmortarres.md)
- [docs/analysis/tunables.md](../../analysis/tunables.md) §3
