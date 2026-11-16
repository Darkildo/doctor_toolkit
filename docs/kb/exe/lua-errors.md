---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 49]
sources: ["docs/analysis/live-link.md §11.2", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, lua, errors, journal]
updated: 2026-11-16
---
# Ошибки Lua: защищённые вызовы и стек

Адреса — виртуальные, издание PC 1.0.789.0. Все защищённые вызовы Lua проходят через `luaD_rawrunprotected`; игра ошибки скриптов молча глотает. Журнал модуля перехватывает три вызова этой функции и вход `luaG_errormsg`, чтобы снять сообщение и стек.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00635AB0` | `luaD_rawrunprotected(L, f, ud)`, `cdecl`; возвращает код Lua | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00D3D380` | `setjmp` внутри `luaD_rawrunprotected` | live-link.md §11.2 |
| `0x00BA0B4C` | `longjmp` | live-link.md §11.2 |
| `0x006365A0` | вызов `luaD_rawrunprotected` в `luaD_pcall` — все защищённые вызовы (перехват журнала) | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x006365B9` | место после вызова, где восстанавливается `L->ci` | live-link.md §11.2 |
| `0x006366F0` | `luaD_protectedparser` | live-link.md §11.2 |
| `0x00636722` | вызов `luaD_rawrunprotected` в парсере — ошибки компиляции (перехват журнала) | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00636440` | `lua_resume` | live-link.md §11.2 |
| `0x00636488` | вызов `luaD_rawrunprotected` в `lua_resume` — ошибки сопрограмм (перехват журнала) | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x006338A0` | `luaG_errormsg` (`L` в `ESI`): через неё идут все ошибки выполнения; журнал переходит с её первых пяти байт | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x006338A5` | продолжение `luaG_errormsg` после перехваченных пяти байт | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00627BD0` | `luaL_error` (зовёт `lua_error`) | live-link.md §11.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00635A80` | `luaD_throw` | live-link.md §11.2 |
| `0x00632AA0` | `currentline`: по ней выведены поля `CallInfo` и `Proto` | live-link.md §11.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00632D40` | `funcinfo`: по ней выведены поля замыкания и `Proto` | live-link.md §11.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00632FB0` | `lua_getinfo` (`"Sl"` в `print` игры) | live-link.md §11.1 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §11.2 «Ошибки Lua»
