---
type: question
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 74, 87]
sources: ["docs/scripts.md §9", "docs/analysis/lua-api.md §7", "docs/analysis/game-bench.md §35"]
tags: [question, lua, police, vehicles]
updated: 2026-11-16
---
# Полиция и машины из скрипта Lua не проверены

Этап 74 разобрал функции в коде. `GetFootPoliceIntensity` (`0x787470`) и `GetCarPoliceIntensity` (`0x7874B0`) одинаковы: `0.0`, если поле `CrimeManager(*0x112AA1C)+0x2C74` не нуль, иначе `nil` — силы погони они не возвращают. Функции транспорта читают машину игрока (`0x7FF880`); пешком все дают 0, `GetVehicleSpeed()` — `0, false`. На стенде проверено только это состояние: без погони и пешком.

## Следующий шаг

На стенде создать погоню и посадить игрока в машину (сохранение или сценарий ввода), вызвать функции через `live lua` и записать значения, в том числе `GetVehicleType` для разных машин и `GetChaseVehicleStats`.

Запланировано: [Этап 87: Полиция и закон](../research/stage-87.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Полиция и закон](../systems/police.md)
- [Транспорт и трафик](../systems/vehicles.md)
- [docs/scripts.md](../../scripts.md) §9
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
