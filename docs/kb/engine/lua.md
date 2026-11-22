---
type: engine
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 16, 28, 34, 38, 45, 49, 74]
sources: ["docs/formats/lua.md §1", "docs/formats/lua.md §3", "docs/formats/mission-logic.md §2", "docs/formats/mission-logic.md §3", "docs/scripts.md §1", "docs/analysis/live-link.md §5", "docs/analysis/live-link.md §11.1", "docs/analysis/live-link.md §11.2", "docs/logic-graph.md §9.3", "docs/analysis/lua-api.md §1", "docs/analysis/game-bench.md §35", "data/gf2/lua_api.toml"]
tags: [lua, scripts, bytecode, audio]
updated: 2026-11-20
---
# Lua в игре

Lua 5.0.2 вкомпилирован в `godfather2.exe` и служит только скриптовым движком звука и музыки. Логика миссий — не Lua, а объекты [SimGroup](simgroup-objects.md); но скрипт подключён к той же [шине сообщений](message-bus.md) и к переменным движка.

## Что известно

- Ресурсы `lua` — 30 штук, все в `gf2/globals_global`: байт-код Lua 5.0 (`\x1bLua`, версия `0x50`, little-endian, размеры 4/4/4, `SIZE_OP/A/B/C` = 6/8/9/9), числа — `float` (4 байта). Отладочная информация не вырезана. Ресурс может быть и исходным текстом: `luaL_loadbuffer` различает по первому байту.
- `LuaManager` — `*0x012233F4` (конструктор `0x0043E140`): состояние `+0x18` создаётся с симуляцией (`0x0043E790`), `+0x1C` — при старте приложения (`0x0043E5D0`). Все потребители работают с `+0x18`. В главном меню состояния нет: найденный скрипт там роняет игру (`0x00480E03`).
- Ядро: `lua_open` `0x00627840`, `luaL_loadbuffer` `0x00629280`, `luaD_pcall` `0x00636570`, `lua_dobuffer` `0x006293C0`, `luaD_rawrunprotected` `0x00635AB0`, `luaG_errormsg` `0x006338A0`. Соглашения о вызове нестандартные (LTCG): у `luaL_loadbuffer` буфер в `ECX`, имя куска в `EAX`.
- Библиотеки: base (с `coroutine`), table, string, math, `tolua`; io, os, debug, loadlib нет. `loadfile`, `dofile`, `require` читают `.lua` с диска через `fopen` от каталога игры.
- Функции движка: 74 глобальные функции C, 37 методов семи таблиц (`GroupManager`, `Group`, `Variable`, `UInt32`, `Int32`, `Base`, `MessageReceiver`), 6 функций встроенного Lua и 78 библиотечных — [полный список](../exe/lua-functions.md) с адресами и подписями (`data/gf2/lua_api.toml`). У всех функций игры подпись прочитана в коде, 63 проверены на стенде.
- Сообщения: `SendMessage(имя [, тип, данные])` (`0x004C82C0`, синхронный) шлёт указатель на число, текст или userdata; `SendMessageEx` возвращает ответ слушателя; обработчик получателя — `fn(self, имя, имяТипа, данные)`, данные — сырой указатель.
- Время и целые: `GetSimTime()` (`0x0046A850`), `VariableGet` и `Group:GetMemberCount` возвращают объекты `UInt32`/`Int32`, а не числа Lua; число даёт `Int32ToNumber`. Таймеры: `CreateTimer(имяФункции, oneShot [, useRealTime])`, обработчик получает номер таймера.
- Объекты: `Base(строка)` в этой сборке объектов игры не находит (сломан разбор GUID, `0x0046B4D0`); методы групп с пустой ссылкой роняют игру (`0x00469F85`).
- Звук: `PlayEmx` (`0x00436A90`; имя нужно с `.emx`), `PlayEarsAudioStream` (`0x00438D10`; имя с `.stx`); ненайденное — light userdata 0, истинное значение. `GetPlayerPosition(0)` (`0x00469230`) — номер игрока обязателен. `EclipseLog` — пустая (`0x0043E5C0`). `SetWatchValue` и `ClearWatchValue` в exe нет.
- В exe есть три фрагмента Lua исходным текстом: пустые `debug_print`/`debug_screenprint`; `gFunctionRegistry`, `RegisterFunction`, `CallLuaFunction`; обвязка `MessageReceiver`.
- Запуск: объекты класса `LuaAudioScript` (хэш `0x07B6C269`, 29 в `globals_global`): атрибут 0 — флаги (бит 0 — запустить при загрузке), 1 — стартовое сообщение, 2 — стоповое, 3 — GUID ресурса `lua`. Чанк выполняется в `0x004CB0C0`, затем вызывается `startup`; стоп и деструктор зовут `shutdown`. С битом 0 скрипт идёт прямо при чтении атрибутов (`0x00480D01` → `0x00480D90`): объекты после него ещё не созданы.
- `print` — своя функция `0x0046BCE0`: зовёт указатель `0x0110A6C0` `(скрипт, строка, what, текст)`, в выпущенной сборке там пустая `0x0046BFC0` — вывод теряется.
- Ошибки: все вызовы скриптов идут через `pcall` без обработчика и только сравнивают код с нулём — ошибки не видны, игра не падает, скрипт перестаёт работать с места ошибки. Коды: 1 выполнение, 3 синтаксис, 4 память, 5 ошибка обработчика. [Модуль связи](live-module.md) перехватывает три вызова `luaD_rawrunprotected` и `luaG_errormsg` и пишет ошибки со стеком; его `live lua` выполняет строку в состоянии `+0x18`.
- Тулкит: компилятор — перенос Lua 5.0.3 на Rust (чанк побайтно как у `luac` с `float`); декомпилятор — все 30 скриптов перекомпилируются побайтно, со строками отладочной информации; ВМ Lua 5.0 (`resource/lua/vm.rs`, с сопрограммами) с макетом API игры (`script::sim`) — для прогона логики без игры.

## Что неизвестно

- [Объект, который `Base` способна найти, и группы на настоящем объекте](../questions/lua-groups-and-objects.md).
- [Что лежит по указателю в данных сообщений игры](../questions/lua-message-data.md).
- [Полиция при погоне и транспорт за рулём](../questions/lua-police-and-cars.md).
- В прогоне без игры сообщения скрипта уходят в шину после возврата из Lua, а в игре `SendMessage` синхронный; время и переменные в макете — числа, а не объекты целых.

## Подробнее

- [Скрипты Lua: формат, компилятор, декомпилятор](../../formats/lua.md)
- [Логика миссий и Lua](../../formats/mission-logic.md) §2–§3
- [Визуальные скрипты](../../scripts.md) §1
- [Живая связь](../../analysis/live-link.md) §5, §11.1, §11.2
- [Граф логики](../../logic-graph.md) §9.3
- [Lua-API движка](../../analysis/lua-api.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
