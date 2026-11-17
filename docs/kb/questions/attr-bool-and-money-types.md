---
type: question
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [73, 79]
sources: ["data/gf2/attr_types.toml", "research/attr_types.py", "docs/analysis/tunables.md §2", "data/gf2/class_behavior.toml"]
tags: [question, simgroup, types]
updated: 2026-11-16
---
# Типы `Bool` и `Money` у атрибутов — догадки

В `data/gf2/attr_types.toml` 6 атрибутов помечены `Bool` (названные целые атрибуты, у которых во всех файлах игры встречаются только 0 и 1: `Destructible` 0, 3, 5, `Door` 4, `Entity` 3, 4) и 4 — `Money` (`Bank` 0 и 1, `NPC` 25 и 26). Все десять — `guess`: значение не по умолчанию «1» может быть и номером варианта, а суммы в игре не проверялись.

## Что выяснено (этап 79)

Из шести `Bool` код подтвердил четыре: `Destructible` 0, 3, 5 (читатель `0x008B9590`: «не ноль» ставит биты 0, 1, 4 слова `+0x18C`) и `Door` 4 (`0x00795C10`: бит `0x1000000`). Они теперь `exe`. Ещё семь `Bool` добавлены по коду (`CSystemCommands` 4, `Door` 17, `NPCRequester` 2, `TriggerBase` 9, `UIHudMiniMapMarker` 7, 8, 25). `Entity` 3 и 4 остались догадками: читатель сущности `0x0046F160` не прочитан. Типы `Money` (`Bank`, `NPC`) этот этап не проверял — классы денег разбирает этап 75.

## Следующий шаг

Прочитать читатель `Entity` `0x0046F160` (атрибуты 3 и 4); `Money` — по итогам этапа 75.

Сделано частично: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Точное деление значений атрибутов на int, float, hash и bool](simgroup-value-kinds.md)
- [Экономика](../systems/economy.md)
- [Этап 73](../research/stage-73.md)
