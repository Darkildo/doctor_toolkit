---
type: question
status: hypothesis
confidence: data
editions: ["1.0.789.0"]
stages: [72, 75]
sources: ["docs/analysis/tunables.md §3"]
tags: [question, items, economy]
updated: 2026-11-16
---
# `Item`: суммы подбираемых денег и лечение аптечки

`Item` «Cash» атрибут 4 (100…4000) — вероятно, суммы подбираемых денег; объекты почти все в локальных потоках заведений. «Health Pack» 4 (500, в мультиплеере 1500) — вероятно, сколько лечит. В список параметров не включены.

## Следующий шаг

Проверить на стенде: изменить атрибут 4 у предмета рядом с игроком; разобрать, как выдать предмет из SimGroup или Lua.

Запланировано: [Этап 75: Предметы, деньги, оружие из логики; выбор как меню](../research/stage-75.md).

## Связанное

- [Класс `Item`](../classes/Item.md)
- [Экономика: деньги, заведения, награды](../systems/economy.md)
- [docs/analysis/tunables.md](../../analysis/tunables.md) §3
