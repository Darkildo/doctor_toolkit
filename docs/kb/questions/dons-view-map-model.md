---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 94, 84]
sources: ["docs/analysis/queens.md §7", "docs/formats/wgi.md §1"]
tags: [question, ui, map]
updated: 2026-11-16
---
# Модель большой карты «Взгляда дона»

Остров мода на большой карте «Взгляда дона» не виден: модель карты не найдена и не разобрана.

Зацепка этапа 84: у каждого города четвёртый раздел мировой сетки (маска LOD 8 — «вид карты») держит одну ячейку — поток карты «Взгляда дона»: `gf2\queens\global\dons_view\dv_zonestream`, `gf2\cuba\global\dons_view\dons_view`, `gf2\miami\global\dons_view\map_stream` ([формат `wgi`](../../formats/wgi.md) §1). Содержимое этих потоков не разбиралось.

## Следующий шаг

Разобрать потоки `global/dons_view/…` (модель карты, скорее всего, в них) и способ добавить на неё новую сушу.

Запланировано: [Этап 94: Интерфейс](../research/stage-94.md).

## Связанное

- [Интерфейс: APX (Apt), шрифты, строки](../engine/ui-apt.md)
- [Мир: города, зоны, время суток](../systems/world.md)
- [docs/analysis/queens.md](../../analysis/queens.md) §7
