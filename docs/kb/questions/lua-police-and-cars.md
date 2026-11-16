---
type: question
status: unknown
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 74, 87]
sources: ["docs/scripts.md §9"]
tags: [question, lua, police, vehicles]
updated: 2026-11-16
---
# Полиция и машины из скрипта Lua не проверены

Функции Lua для полиции и машин в игре не проверялись: на стенде игрок вне машины и без погони, поэтому результат вызовов не виден.

## Следующий шаг

Разобрать эти функции в exe (аргументы, побочные эффекты); на стенде создать условия — погоню или посадку в машину — и проверить вызовы.

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md), [Этап 87: Полиция и закон](../research/stage-87.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Полиция и закон](../systems/police.md)
- [Транспорт и трафик](../systems/vehicles.md)
- [docs/scripts.md](../../scripts.md) §9
