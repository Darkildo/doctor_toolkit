---
type: exe
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58, 66, 69, 70, 17]
sources: ["docs/analysis/renderer.md §7", "docs/analysis/renderer.md §10.1", "docs/analysis/renderer.md §10.7", "docs/analysis/renderer.md §10.9", "docs/formats/characters.md §2"]
tags: [exe, shaders, render]
updated: 2026-11-16
---
# Байткод шейдеров в exe: смещения

Числа ниже — смещения в файле `godfather2.exe` (источник называет диапазон шейдеров `0xA7BE18`–`0xD09A48` смещениями файла, инструмент `research/d3d9_shaders.py` принимает смещение файла), а не виртуальные адреса; издание PC 1.0.789.0. 943 шейдера `ps_3_0`/`vs_3_0` с таблицами констант лежат в `exe`; документы называют отдельные шейдеры по их смещению. По `launch-flags.md`, виртуальный адрес = смещение в файле + `0x400000`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00A7C0F0` | данные: шейдер `gf_basic` (материалы `Basic`) | renderer.md §10.7 |
| `0x00A7DC18` | данные: шейдер `gf_basic_tint_reflect` | renderer.md §10.7 |
| `0x00A7FB30` | данные: шейдер `gf_building` | renderer.md §10.7 |
| `0x00A82198` | данные: шейдер `gf_building_detail` | renderer.md §10.7 |
| `0x00A82EF0` | данные: шейдер `gf_building_detail` без блеска | renderer.md §10.7 |
| `0x00A814F0`, `0x00A81730` | данные: вершинные шейдеры, задающие UV двух слоёв (основной — UV меша, детальный — `uv · g_DetailUVScaleAndOffset`) | renderer.md §10.7 |
| `0x00A82C48` | данные: шейдер `gf_building_detail_depth` | renderer.md §10.7 |
| `0x00A86AE0` | данные: шейдер `DepthSpecExpNxy12PS` | renderer.md §10.7 |
| `0x00A87B78` | данные: шейдер `DepthSpecExp…LowResPS` | renderer.md §10.7 |
| `0x00A8DB48` | данные: шейдер `gf_char_eye` | characters.md §2 |
| `0x00A8E3F8` | данные: шейдер `gf_char_eye`, низкое качество | characters.md §2 |
| `0x00A9F7B8` | данные: шейдер `gf_char_hair_aniso` | characters.md §2 |
| `0x00AA2F98` | данные: шейдер `gf_char_hair_aniso_tint` | characters.md §2 |
| `0x00ACAFE8` | данные: шейдер `gf_normal_spec*` | renderer.md §10.7 |
| `0x00ACC138` | данные: шейдер `gf_overlay_vertex_ao` | renderer.md §10.7 |
| `0x00ACCF40` | данные: шейдер `gf_pano` | renderer.md §10.7; renderer.md §10.9 |
| `0x00ACEA68` | данные: освещённый проход `gf_basic` (формула кадра) | renderer.md §10.1 |
| `0x00ADDAD0` | данные: шейдер `WaterOcean*`, простой вариант | renderer.md §10.7 |
| `0x00AE27A0` | данные: шейдер `WindowGlass` | renderer.md §10.7 |
| `0x00AE9138` | данные: шейдер `WindowGlow` | renderer.md §10.7; renderer.md §10.9 |
| `0x00AEA4D8` | данные: шейдер `WindowParallax` (ночное свечение, назван вместе с `WindowGlow`) | renderer.md §10.9 |
| `0x00C46698` | данные: проходы размытия блума (`gWeightBloom`) | renderer.md §10.9 |
| `0x00C59B00` | данные: шейдер `AlchemyLUTShader` (и ещё пять — до шести таблиц цвета) | renderer.md §10.9 |
| `0x00C5B9A0` | данные: композит блума | renderer.md §10.9 |
| `0x00CC4300` | данные: шейдеры буфера света (и соседние) | renderer.md §10.7 |
| `0x00CC4D00` | данные: шейдер `DeferredPointLight` | renderer.md §10.9 |
| `0x00CC7F60` | данные: шейдер `DeferredSpotLight` | renderer.md §10.9 |
| `0x00CCD070` | данные: проход солнца, читающий буфер ослабления тени | renderer.md §10.9 |
| `0x00CD5930` | данные: солнце в буфере света | renderer.md §10.1 |
| `0x00CD68A0` | данные: солнце в буфере света, с тенью | renderer.md §10.1 |
| `0x00CDCCA8` | данные: шейдеры сбора каскадов тени в экранный буфер ослабления (и следующие) | renderer.md §10.9 |
| `0x00D04930` | данные: рассеянный свет — квадратичная форма трёх матриц × `g_lightColorSH` | renderer.md §10.1 |
| `0x00D083B0` | данные: шейдер `DeferredNormals` | renderer.md §10.7 |

## Подробнее

- [Рендер](../../analysis/renderer.md) §7, §10.1, §10.7, §10.9
- [Персонажи](../../formats/characters.md) §2
