---
type: research
status: planned
confidence: guess
editions: ["1.0.789.0"]
stages: [74]
sources: ["план исследований: решение владельца от 2026-11-16"]
tags: [research, planned]
updated: 2026-11-16
---
# Этап 74: Lua-API движка

- Состояние: запланировано.
- Целевое издание: только PC 1.0.789.0.

## Открытые вопросы этапа

- [Что лежит в данных сообщения (`data`) и как читать их из Lua](../questions/lua-message-data.md)
- [`PlayEarsAudioStream` из своего скрипта не проверен в игре](../questions/lua-play-ears-audio-stream.md)
- [Откуда скрипту Lua брать объекты игры (группы, функция `Base`)](../questions/lua-groups-and-objects.md)
- [Полиция и машины из скрипта Lua не проверены](../questions/lua-police-and-cars.md)
- [Стоповое сообщение скрипта (`shutdown`) в игре не проверялось](../questions/lua-shutdown-message.md)
- [Сопрограммы (`coroutine`) в ВМ тулкита не поддержаны](../questions/lua-coroutines-in-toolkit-vm.md)
- [`SetTimeOfDay` и `UIFanfare` из скрипта в кадре не сверялись](../questions/lua-settimeofday-uifanfare-in-frame.md)
- [Используется ли состояние `+0x1C` менеджера Lua](../questions/logic-manager-state-1c.md)
- [Как таймеры вызывают функцию Lua по имени](../questions/lua-timer-function-by-name.md)

## Подробнее

- [Карта исследований](../maps/research.md)
- [Как обновлять базу](../README.md)
