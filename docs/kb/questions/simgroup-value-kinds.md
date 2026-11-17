---
type: question
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 73, 79]
sources: ["docs/formats/simgroup.md §8", "docs/formats/simgroup.md §4", "data/gf2/attr_types.toml", "research/attr_types.py", "data/gf2/class_behavior.toml", "docs/analysis/class-behavior.md §4"]
tags: [question, simgroup, types]
updated: 2026-11-16
---
# Точное деление значений атрибутов на int, float, hash и bool

Значения атрибутов не несут типа: вид каждого атрибута выведен из файлов игры (`data/gf2/simgroup_schema.txt`). Точное деление int / float / hash / bool не установлено.

Этап 73 добавил поверх видов **смысл** там, где он виден по данным или коду (`data/gf2/attr_types.toml`, 468 атрибутов): ссылка на объект класса, ресурс типа, значение [перечисления](../enums/index.md), секунды. Это подтверждает вид у части атрибутов: 23 атрибута вида `hash` — хэши имён семей (15) и городов (8), все значения найдены в списках; 17 целых — номера заведений. `Bool` выставлен только шести атрибутам и только как догадка ([вопрос](attr-bool-and-money-types.md)): по данным флаг не отличить от перечисления с одним значением. Для остальных атрибутов вопрос открыт.

## Что выяснено (этап 79)

Вид значения **по коду читателя** записан у 462 атрибутов 45 классов и 5 общих пакетов (`kind` в `data/gf2/class_behavior.toml`, колонка «Вид по коду» в заметках классов): `float` 89, `msg_in` и `msg_out` по 72, `guid` 68, `int` 65, `flags` 33, `hash` 29, `enum` 13, `bool` 12, `string` 6, `vec3` 2, `matrix` 1.

С видом, выведенным из файлов, расходятся четыре (`research/class_behavior.py kinds`): `InterestingObject` 11 и `SendStimulus` 3 — флаги со старшими битами, принятые за хэш; `UIHudMiniMapMarker` 6 — цвет; `GenomeFilter` 8 — не дочитан. 67 атрибутов в файлах игры не записаны ни разу — их вид известен только по коду.

Общее правило: атрибут, которого в пакете нет, читатель получает как 0 или пустую строку (`0x0043AFF0`), значение конструктора при этом затирается.

## Следующий шаг

Остальные классы (около 200) — следующими волнами тем же способом: `research/ghidra/ClassDump.java` по фабрике класса, чтение читателя, запись в `class_behavior.toml`. `simgroup_schema.txt` остаётся выводом из файлов; вид по коду — в `class_behavior.toml`.

Сделано частично: [Этап 79: Поведение классов SimGroup, волна 1](../research/stage-79.md).

## Связанное

- [Классы SimGroup](../classes/index.md)
- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md)
- [docs/formats/simgroup.md](../../formats/simgroup.md) §4, §8
