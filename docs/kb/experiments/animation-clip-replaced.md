---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 26]
sources: ["docs/analysis/game-bench.md §15"]
tags: [animation, bank, idle]
updated: 2026-11-16
---
# Заменённые клипы анимации видны на игроке

Проверка этапа 26. Тень `gf2/globals_global.str`; `anim import` заменил все десять клипов входа `idle`
банка `c0e2841d` анимацией «руки вверх». Сценарий: профиль с сохранением на террасе, `await
resume-selected`, `Return`, `await hud`, снимки через 5, 9 и 13 с, `hold w 2`, снимки после ходьбы.

Результат: игрок стоит с поднятыми и покачивающимися руками; после ходьбы встаёт в обычную стойку
другого клипа. 2,5 мин от запуска до первого снимка.

Связанный опыт с тем же входом `idle` — правка графа анимации
([граф анимации](anim-graph-edit.md)).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §15
- [Формат персонажей](../../formats/characters.md) §6.7
