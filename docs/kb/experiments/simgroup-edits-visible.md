---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 6, 11, 24]
sources: ["docs/analysis/game-bench.md §13.3", "docs/analysis/game-bench.md §13.12"]
tags: [simgroup, metamodel, level]
updated: 2026-11-16
---
# Правки SimGroup и metamodel видны в кадре

Этап 24: тени с копией `cp_010_hotel_interior_zonestream.str`, правки с `--direct`, снимок из точки
сохранения через 5 с после появления управления, сравнение с контрольным прогоном `c0`. Объект —
тележка 97 группы № 89 (metamodel `daa0769a`).

| Прогон | Правка | Результат |
|---|---|---|
| A | `simgroup-set … 89 97 --position 181.4,22.4,-24.8 --rotation 0,45,0` (выше на 1,3 м, поворот 45°) | видно: тележка висит у камеры, повёрнута |
| A | `level duplicate … 89 97 --offset=-4.04,0,-0.88` | видно: вторая тележка слева |
| A | `simgroup-set … 89 107 --scale 1.6,1.6,1.6` | видно: дубликат крупнее оригинала |
| B | `level remove … 89 97` | видно: справа у перил пусто |
| B | `level add … 89 97 --model daa0769a --position 177.36,21.1,-25.68` | видно: новый объект слева |
| B | metamodel `daa0769a`: матрица дочерней части `plate_stack02_des` (`CMD_LoadMatrix`) выше на 0,8 м (`logic-export` → JSON → `logic-import`) | видно: пачка тарелок висит над тележкой |

Игра не падает, управление работает. Прогоны A, B и контроль повторены на исходных файлах игры —
результат тот же.

Ограничение: смена ресурса в списке `resources` metamodel (`ModelPart.Model` → другой меш)
`logic-import` не записывает («no changes»): меняются только свойства и условия; ссылки на ресурсы —
через `simgroup-set --model` или клон.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §13.3, §13.12
- [Формат SimGroup](../../formats/simgroup.md)
- [Формат metamodel](../../formats/metamodel.md)
- [Уровни](../../levels.md)
