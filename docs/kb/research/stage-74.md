---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [74]
sources: ["план исследований: решение владельца от 2026-11-16", "docs/analysis/lua-api.md §1", "docs/analysis/game-bench.md §35", "data/gf2/lua_api.toml", "research/lua_api.py"]
tags: [research, lua, scripts]
updated: 2026-11-16
---
# Этап 74: Lua-API движка

- Состояние: сделан 2026-11-16.
- Зависит от: [этап 72](stage-72.md), [этап 73](stage-73.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Найдены все регистрации функций Lua в exe: 195 функций (74 глобальные функции C, 37 методов таблиц, 6 функций встроенного Lua, 78 библиотечных) и 16 таблиц — `data/gf2/lua_api.toml`, [полный список](../exe/lua-functions.md); поиск и сверка — `research/lua_api.py`.
- У всех 117 функций игры прочитаны аргументы, результаты и побочные эффекты; 63 проверены на стенде ([опыт](../experiments/lua-api-on-bench.md)).
- Описаны данные сообщений со стороны Lua, `Base` и группы, потоки EARS, таймеры, целые, полиция и транспорт, второе состояние Lua, запуск и `shutdown` — [Lua-API движка](../../analysis/lua-api.md).
- ВМ тулкита получила сопрограммы; макет API и узлы визуальных скриптов приведены к найденному, новых узлов 15.

## Главные находки

- `Base` от строки не находит объекты игры (сломан разбор GUID), методы групп с пустой ссылкой роняют игру — [падения](../exe/known-crashes.md).
- Время, переменные и счётчики — объекты `UInt32`/`Int32`, а не числа Lua; узлы «Game time» и «Get game variable» до этапа отдавали их как есть.
- `GetPlayerPosition` без номера игрока всегда `nil`.
- Функции полиции не возвращают силу погони: только `nil` или 0.

## Вопросы этапа

- Решены: [`PlayEarsAudioStream`](../questions/lua-play-ears-audio-stream.md), [`shutdown`](../questions/lua-shutdown-message.md), [сопрограммы](../questions/lua-coroutines-in-toolkit-vm.md), [состояние `+0x1C`](../questions/logic-manager-state-1c.md), [таймеры по имени](../questions/lua-timer-function-by-name.md).
- Частично: [данные сообщений](../questions/lua-message-data.md), [объекты и группы](../questions/lua-groups-and-objects.md), [полиция и машины](../questions/lua-police-and-cars.md).
- Не тронут: [`SetTimeOfDay` и `UIFanfare` в кадре](../questions/lua-settimeofday-uifanfare-in-frame.md).

## Правила этапа

Этап обязан обновить базу знаний: вопросы получают новый статус, появляются новые заметки, у каждого утверждения есть источник. См. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
