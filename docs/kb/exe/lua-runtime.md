---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 16, 34, 74, 80]
sources: ["docs/formats/mission-logic.md §2.1", "docs/formats/mission-logic.md §3", "docs/analysis/live-link.md §5", "docs/analysis/live-link.md §11.2", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "docs/analysis/lua-api.md §1", "docs/analysis/lua-api.md §8", "docs/analysis/lua-api.md §9", "docs/analysis/asi-platform.md §3"]
tags: [exe, lua, scripts]
updated: 2026-11-16
---
# Lua 5.0 в exe: состояние, загрузка и вызов

Адреса — виртуальные, издание PC 1.0.789.0. Lua 5.0 вкомпилирован в `exe` с нестандартными соглашениями о вызовах (аргументы в регистрах). Синглтон `LuaManager` держит два состояния; скрипты игры работают с тем, что в `+0x18`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x012233F4` | данные: синглтон `LuaManager` (указатель); `+0x18` — `lua_State` скриптов игры | mission-logic.md §2.1; live-link.md §5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0043E140` | конструктор `LuaManager` | mission-logic.md §2.1 |
| `0x00E3105C` | данные: таблица методов `LuaManager` | mission-logic.md §2.1 |
| `0x0043E5D0` | создание состояния `+0x1C` при старте приложения | mission-logic.md §2.1 |
| `0x0043E790` | создание состояния `+0x18` вместе с объектом симуляции | mission-logic.md §2.1; mission-logic.md §3 |
| `0x00680130` | вызывает `0x0043E790` (создание состояния с симуляцией) | mission-logic.md §3 |
| `0x00627840` | `lua_open` | mission-logic.md §2.1 |
| `0x0043E5DC`, `0x0043E79C` | два места вызова `lua_open` | mission-logic.md §2.1 |
| `0x00625CA0` | `lua_pushstring` | mission-logic.md §2.1 |
| `0x00625D70` | `lua_pushcclosure` | mission-logic.md §2.1 |
| `0x00626030` | `lua_settable` | mission-logic.md §2.1 |
| `0x00628260` | `luaL_openlib` | mission-logic.md §2.1 |
| `0x004AEB70` | обход массива `{имя, функция}` при регистрации функций | mission-logic.md §2.1 |
| `0x00629280` | `luaL_loadbuffer`: буфер в `ECX`, имя куска в `EAX` | mission-logic.md §2.1; live-link.md §5; crates/doctor-toolkit-live/src/addr.rs |
| `0x006290C0` | `luaL_loadfile` (`fopen` с диска) | mission-logic.md §2.1 |
| `0x006293C0` | `aux_do` (`luaL_dobuffer`, `lua_dobuffer` игры) | mission-logic.md §2.1; live-link.md §5 |
| `0x006292E0` | `callalert`: `_ALERT` или `fprintf(stderr)` | mission-logic.md §2.1; live-link.md §11.2 |
| `0x00636570` | `luaD_pcall`: `L` в `ESI`, обработчик ошибок в `EDX` | mission-logic.md §2.1; live-link.md §5; crates/doctor-toolkit-live/src/addr.rs |
| `0x006261E0` | `f_call` для `lua_pcall`: `ud` — `{StkId func; int nresults}` | live-link.md §5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00626200` | `lua_pcall` | live-link.md §11.2 |
| `0x004CB0C0` | выполнение ресурсного скрипта: `luaL_loadbuffer` и вызов | mission-logic.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x0062E4C0` | ещё одно место использования `luaL_loadbuffer` (названо вместе с `0x004CB0C0`) | crates/doctor-toolkit-live/src/addr.rs |
| `0x00480D90` | `LuaAudioScript`: берёт `LuaManager` и состояние `+0x18` | live-link.md §5; crates/doctor-toolkit-live/src/edition.rs |
| `0x00480D01` | место запуска скрипта объекта (→ `0x00480D90`) | mission-logic.md §3 |
| `0x00468F40` | `CallLuaFunction`: вызов скрипта игрой; при ошибке возвращает `false` | live-link.md §11.2 |
| `0x004CE610` | `MessageReceiverReceive`: вызов скрипта по сообщению | live-link.md §11.2 |
| `0x0043E500`, `0x00437A80`, `0x00467440`, `0x00468EA0` | прочие вызовы скриптов игрой через `luaD_pcall` | live-link.md §11.2 |
| `0x00469230` | Lua `GetPlayerPosition` | live-link.md §3; crates/doctor-toolkit-live/src/edition.rs |
| `0x01223474` | данные: объект позиции Lua (используется `GetPlayerPosition`) | crates/doctor-toolkit-live/src/edition.rs |
| `0x0043E5C0` | `EclipseLog`: `mov eax, 1; ret` | mission-logic.md §3 |
| `0x004211E0` | поиск ресурса (скрипта) по GUID в момент запуска | mission-logic.md §3; live-link.md §12.3 |
| `0x00625740`, `0x006259B0`, `0x00625A30`, `0x00625A70`, `0x00625B60` | `lua_type` (индекс в `EDX`, `L` в `EAX`), `lua_tonumber`, `lua_toboolean`, `lua_tostring`, `lua_touserdata` | lua-api.md §1 |
| `0x00625C20`, `0x00625C40`, `0x00625DF0`, `0x00625E10`, `0x00625F40` | `lua_pushnil`, `lua_pushnumber`, `lua_pushboolean`, `lua_pushlightuserdata`, `lua_newtable` | lua-api.md §1 |
| `0x00627BD0`, `0x00627C90`, `0x00627E50`, `0x00638A40` | `luaL_error`, `luaL_newmetatable`, проверка userdata по имени метатаблицы, `lua_newuserdata` | lua-api.md §1 |
| `0x00633D60`, `0x00633CB0`, `0x006362D0` | `luaV_settable`, `luaV_gettable`, `luaD_call` | lua-api.md §1 |
| `0x00480AF0`, `0x00480D40`, `0x00480A00` | `LuaAudioScript`: чтение атрибутов, обработчик стартового и стопового сообщений, деструктор (зовёт `shutdown`) | lua-api.md §9 |
| `0x00468E10` | `RegisterScriptName`: имя скрипта в объект `+0x84` | lua-api.md §9 |
| `0x0110AA98`, `0x0110A6F0`, `0x011289F8` | данные: три встроенных текста Lua (`debug_print`; `RegisterFunction` и `CallLuaFunction`; обвязка `MessageReceiver`) | lua-api.md §9 |
| `0x004CB170` | обёртка загрузки чанка в состояние `+0x1C`: ни вызовов, ни ссылок | lua-api.md §8 |

## Что неизвестно

Состояние `+0x1C`: функции в него регистрируются, чанки скриптов не загружаются (этап 74); доступ к нему через другой указатель не исключён.

## Подробнее

- [ASI-платформа](../../analysis/asi-platform.md) §3 — связки LuaManager (`0x0043E2E0`), через которые регистрируются функции; адреса: [платформа нативных модов](native-mods.md)
- [Логика миссий](../../formats/mission-logic.md) §1, §3
- [Живая связь](../../analysis/live-link.md) §5 «Lua»
- [Формат Lua](../../formats/lua.md)
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](lua-functions.md)
