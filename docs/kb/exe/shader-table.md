---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 17, 58, 66]
sources: ["docs/formats/characters.md §2", "data/gf2/shaders.txt", "docs/analysis/renderer.md §1", "docs/analysis/renderer.md §7", "docs/analysis/launch-flags.md §3"]
tags: [exe, shaders, materials]
updated: 2026-11-16
---
# Шейдеры материалов: таблица в exe и обработчики параметров

Адреса — виртуальные, издание PC 1.0.789.0. Материал называет шейдер по CRC-32 имени в нижнем регистре; имена и флаги берутся из таблицы шейдеров в `exe`. 943 скомпилированных шейдера SM 3.0 лежат прямо в `exe`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00E7B200` | данные: таблица шейдеров, записи по 0x24 байта: `*имя`, 6 слов рантайма, функция-обработчик параметров, флаги (0x100 — смешивание по альфе, 1 — освещённый проход) | characters.md §2; data/gf2/shaders.txt |
| `0x009DA3B0` | обработчик параметров: у `g_TintColor0..2`, `g_BaseTintColor`, `WaterColor` и др. возводит RGB в степень 2.2 | characters.md §2; data/gf2/shaders.txt |
| `0x009DA470` | обработчик параметров: то же и у `*UVScaleOffset` заменяет `w` на `1 − w − 1/y` | characters.md §2; data/gf2/shaders.txt |
| `0x00EF1000` | данные: таблица указателей на шейдеры — по четыре варианта качества подряд | characters.md §2 |

## Подробнее

- [Персонажи](../../formats/characters.md) §2 «Шейдеры и материалы»
- [Рендер](../../analysis/renderer.md) §7 «Шейдеры»
