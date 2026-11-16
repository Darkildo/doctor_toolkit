---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 93]
sources: ["docs/formats/rig.md §5"]
tags: [question, rig, names, hash]
updated: 2026-11-16
---
# Входы хэшей рига и наборов костей; 82 неизвестных имени костей

Не известно, из чего считаются хэши рига и наборов костей. 82 имени костей не восстановлены (известные — `data/gf2/bone_names.txt`).

## Следующий шаг

Перебирать кандидатов имён по строкам exe и анатомическим названиям (`research/name_candidates.py`); найти в exe поиск рига по хэшу.

Запланировано: [Этап 93: Новые клипы анимаций](../research/stage-93.md).

## Связанное

- [Тип ресурса `rcb`](../resources/rcb.md)
- [Анимация: скелеты, клипы, граф](../engine/animation.md)
- [docs/formats/rig.md](../../formats/rig.md) §5
