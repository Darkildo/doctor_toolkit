---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [73, 79]
sources: ["data/gf2/enums.toml", "research/enum_values.py"]
tags: [question, simgroup, types, economy]
updated: 2026-11-16
---
# Имена типов зданий (`Building` 1) неизвестны

Атрибут 1 объекта `Building` — число от 1 до 9 (у 9 объектов не задано): тип 1 — 32 здания (номера 101–142), тип 9 — 35 (201–236), тип 6 — 23 (в том числе все карты мультиплеера), типы 2–5, 7, 8 — по 3–6 зданий. Что значит каждое число (рэкет, предприятие, укрепление, банк …), по данным не видно: имён в файлах нет. Поэтому перечисление [`building_type`](../enums/building_type.md) — только числа.

## Следующий шаг

Найти в Ghidra читатель атрибутов `Building` (хэш класса `0x6f1d0554`) и места, где поле типа сравнивается с константами; сверить со строками интерфейса «Взгляда дона». Затем дописать имена в `research/enum_values.py` и пересобрать `data/gf2/enums.toml`.

Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Перечисление `building_type`](../enums/building_type.md)
- [Перечисление `venue`](../enums/venue.md)
- [Класс `Building`](../classes/Building.md)
- [Экономика](../systems/economy.md)
