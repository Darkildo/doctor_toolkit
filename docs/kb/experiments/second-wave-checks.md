---
type: experiment
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 45]
sources: ["docs/analysis/game-bench.md §20", "docs/analysis/game-bench.md §20.2"]
tags: [bench, dialogue, subtitles, trigger, project, sbk]
updated: 2026-11-16
---
# Вторая волна проверок (этап 45): сводка

Быстрый стенд: `SERVER=xvfb DXVK=1`, 1024×768, модуль связи. Два пути в уровень: `-stream gf2 cuba`
(управление через 10–15 с) и сохранение на террасе (управление на 58-й секунде). Позиции, место
проверки, сообщения и Lua — командами `live …`.

| № | Проверка | Результат |
|---|---|---|
| 0 | ходьба после `-stream` | исправлено — [заметка](player-not-walking-after-stream.md) |
| 1 | растянутые треугольники | не воспроизводятся — [заметка](stretched-triangles.md) |
| 2 | новая одежда на игроке | видно — [заметка](character-clothes-and-colours.md) |
| 2 | оружие в руке | тогда не проверено; позже подтверждено — [заметка](weapon-in-hand.md) |
| 3 | диалог и его субтитр | видно: `E` перед Майклом на террасе — диалог с субтитрами; `subtitle … d_mchl_xxx_0001cb9.exa.snu --set en=…` (накладной `GODFATH0.viv`) — в диалоге новый текст |
| 3 | субтитры и поле зрения роликов | тогда не проверено; позже — [ролик igc16](cutscene-igc16-subtitle-fov.md) |
| 4 | дочерний поток | условие найдено — [заметка](child-stream-not-opened.md) |
| 5 | подписи параметров (Family 8, Heart, Reduced Law Costs B) | тогда не проверено; позже частично — [параметры навыков](skill-parameters.md) |
| 6 | действие Blueprints и триггер | видно: `TriggerBox` в 6 м перед игроком (`TriggerBase.4 = doctor_trig_enter`) и граф «сообщение → проба → `boundary_fade_out`»; игрок дошёл — проба `trig_enter`, яркость кадра 106,5 → 1,6 |
| 7 | правка и новые звуки `sbk` | слышно: высота ×1,5 и громкость, новые вариации, новый эмиттер через `PlayEmx("….emx")` |
| 8 | сборка проекта | видно: проект зоны `yuri/high/terrain5`, патч поднял светильник на 3 м и увеличил вдвое, новый объект проекта — копия; после `project build --apply` оба объекта на месте (`live objects`) |

Субтитры включаются в меню паузы → Options → Audio → Subtitles (Accept мышью); по умолчанию Off.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §20
- [Формат звука](../../formats/audio.md) §12
- [Формат диалогов](../../formats/dialogues.md)
- [Проект](../../project.md)
- [Что отложено и не проверено](deferred-and-unverified.md)
