---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 73, 79]
sources: ["docs/formats/simgroup.md §8", "docs/formats/simgroup.md §4", "data/gf2/attr_types.toml", "research/attr_types.py"]
tags: [question, simgroup, types]
updated: 2026-11-16
---
# Точное деление значений атрибутов на int, float, hash и bool

Значения атрибутов не несут типа: вид каждого атрибута выведен из файлов игры (`data/gf2/simgroup_schema.txt`). Точное деление int / float / hash / bool не установлено.

Этап 73 добавил поверх видов **смысл** там, где он виден по данным или коду (`data/gf2/attr_types.toml`, 468 атрибутов): ссылка на объект класса, ресурс типа, значение [перечисления](../enums/index.md), секунды. Это подтверждает вид у части атрибутов: 23 атрибута вида `hash` — хэши имён семей (15) и городов (8), все значения найдены в списках; 17 целых — номера заведений. `Bool` выставлен только шести атрибутам и только как догадка ([вопрос](attr-bool-and-money-types.md)): по данным флаг не отличить от перечисления с одним значением. Для остальных атрибутов вопрос открыт.

## Следующий шаг

Уточнять вид по читателям классов в exe (каким вызовом атрибут читается) и переносить в `data/gf2/simgroup_schema.txt`; смысл — в `MANUAL` скрипта `research/attr_types.py`.

Сделано частично: [Этап 73: Типизация сообщений и атрибутов](../research/stage-73.md). Запланировано: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Классы SimGroup](../classes/index.md)
- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §4, §8
