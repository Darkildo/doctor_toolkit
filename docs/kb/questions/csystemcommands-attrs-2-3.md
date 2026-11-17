---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 79]
sources: ["docs/formats/simgroup.md §8", "godfather2.exe 0x0046F500", "data/gf2/class_behavior.toml"]
tags: [question, simgroup]
updated: 2026-11-16
---
# Что задают атрибуты 2 и 3 `CSystemCommands`

Атрибуты 2 и 3 `CSystemCommands` — int, почти у всех объектов равны 1. Что они задают, не известно.

## Что выяснено (этап 79)

Игра их не читает. Хэш пакета `0xB390B11A` встречается в коде в 46 местах (`research/ghidra/FindConst.java`): 45 открывают пакет и сразу берут атрибут 1 — матрицу (`0x0043AF00(1)`); читатель сущности `0x0046F500` обходит пакет целиком и реагирует на атрибут 1 (матрица и масштаб по осям) и атрибут 4 (не ноль — увеличивает байт `+0x161`). Атрибуты 0, 2 и 3 не берёт никто. В файлах игры атрибуты 2 и 3 равны 1 у 71 340 и 71 247 объектов из 71 355 — по-видимому, поля редактора уровней (догадка).

Сделано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Класс `CSystemCommands`](../classes/CSystemCommands.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §8
