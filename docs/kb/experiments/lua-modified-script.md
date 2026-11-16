---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 16, 24]
sources: ["docs/analysis/game-bench.md §13.8"]
tags: [lua, scripts, globals-global]
updated: 2026-11-16
---
# Изменённый скрипт Lua работает в уровне

Этап 24, проверка этапа 16. Тень с копией `gf2/globals_global.str`; `lua-import` скрипта
`aud_utils.lua` с пробами: `loadfile("doctor_probe_…")` в начале и в конце чанка и в функции
`lua_switch`. Прогон до управления и ещё 60 с, `TRACE_FILES=1`,
`TRACE_SYSCALLS=openat,newfstatat,stat,lstat,statx,access,faccessat,faccessat2`.

Результат:

- игра ищет `doctor_probe_aud_utils_begin.lua` и `doctor_probe_aud_utils_end.lua` — чанк из
  `globals_global` загружен и выполнен до конца;
- 18 разных `doctor_probe_switch_<сообщение>.lua` (`audio_cuba_init`, `aud_init_cuba_m1`,
  `aud_start_cuba_m1_newyears`, `aud_night`, `music_setup`, …) — игра вызывает функцию изменённого
  скрипта в уровне;
- игра не падает.

Вывод: скрипт, собранный компилятором тулкита, работает в игре.

Приём: Wine ищет несуществующий файл через `stat`/`statx`, а не `openat` — в прогоне только с
`openat` проб не видно.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §13.8
- [Формат Lua](../../formats/lua.md) §5
- [Свои скрипты Lua](lua-own-scripts.md)
