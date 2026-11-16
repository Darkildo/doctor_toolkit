---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 70]
sources: ["docs/analysis/renderer.md §10.9"]
tags: [question, render, time-of-day]
updated: 2026-11-16
---
# Атрибуты 2 и 3 `LightingTODMessage` и тень игрока в сумерки

Атрибуты 2, 3 `LightingTODMessage` (0,35/0,35 днём, 0,2/0,4 в сумерки) не разобраны. В сумерки (18:00–21:00 игра держит «луну») тень игрока в Гаване уходит иначе, чем считает редактор, — не разобрано.

## Следующий шаг

Разобрать читатель `LightingTODMessage` и выбор источника тени в сумерки.

## Связанное

- [Время суток, свет и погода](../engine/time-of-day.md)
- [Рендер](../engine/render.md)
- [Класс `LightingTODMessage`](../classes/LightingTODMessage.md)
- [Свет, тональная кривая и таблицы цвета: код и данные](../exe/lighting-tone-lut.md)
- [docs/analysis/renderer.md](../../analysis/renderer.md) §10.9
