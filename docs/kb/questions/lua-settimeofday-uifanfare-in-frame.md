---
type: question
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 45, 74]
sources: ["docs/scripts.md §9", "docs/analysis/game-bench.md §20"]
tags: [question, lua, bench]
updated: 2026-11-16
---
# `SetTimeOfDay` и `UIFanfare` из скрипта в кадре не сверялись

Видимое действие от сообщения скрипта проверено на этапе 45 (`FadeScreenFx` через `boundary_fade_out`). `SetTimeOfDay` и `UIFanfare` по отдельности в кадре не сверялись.

## Следующий шаг

На стенде вызвать каждую функцию из своего скрипта и сравнить кадры до и после.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Сообщение `boundary_fade_out`](../messages/boundary-fade-out.md)
- [Свои скрипты Lua: где они запускаются и что нужно для этого](../experiments/lua-own-scripts.md)
- [docs/scripts.md](../../scripts.md) §1, §9
