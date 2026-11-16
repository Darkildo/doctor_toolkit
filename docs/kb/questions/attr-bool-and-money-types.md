---
type: question
status: hypothesis
confidence: guess
editions: ["1.0.789.0"]
stages: [73, 79]
sources: ["data/gf2/attr_types.toml", "research/attr_types.py", "docs/analysis/tunables.md §2"]
tags: [question, simgroup, types]
updated: 2026-11-16
---
# Типы `Bool` и `Money` у атрибутов — догадки

В `data/gf2/attr_types.toml` 6 атрибутов помечены `Bool` (названные целые атрибуты, у которых во всех файлах игры встречаются только 0 и 1: `Destructible` 0, 3, 5, `Door` 4, `Entity` 3, 4) и 4 — `Money` (`Bank` 0 и 1, `NPC` 25 и 26). Все десять — `guess`: значение не по умолчанию «1» может быть и номером варианта, а суммы в игре не проверялись.

## Следующий шаг

Прочитать читатели этих классов в Ghidra: как поле используется после загрузки (проверка на ноль — флаг; сравнение с константами — перечисление; передача в `AddMoney` `0x008C09A0` — деньги). Перенести подтверждённое в `MANUAL` скрипта `research/attr_types.py` с уверенностью `exe`.

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Точное деление значений атрибутов на int, float, hash и bool](simgroup-value-kinds.md)
- [Экономика](../systems/economy.md)
- [Этап 73](../research/stage-73.md)
