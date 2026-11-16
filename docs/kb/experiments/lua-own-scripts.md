---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 28]
sources: ["docs/analysis/game-bench.md §14", "docs/scripts.md §1"]
tags: [lua, scripts, simgroup, crash]
updated: 2026-11-16
---
# Свои скрипты Lua: где они запускаются и что нужно для этого

Проверки этапа 28. Тени с копией `gf2/globals_global.str` (или `gf2/frontend.str`); правки —
`add-resource`, `level add`, `simgroup-set`, `script-deploy`; сигнал — файловые пробы
`loadfile("doctor_probe_<метка>.lua")` в `files.trace`.

| Прогон | Что | Итог |
|---|---|---|
| меню, 1-й | `LuaAudioScript`, `VariableSwitch`, `VariableOperator` в `gf2/frontend` | падение `0x004420F3` при загрузке меню (нет менеджера переменных) |
| меню, 2-й | только `LuaAudioScript` и `MessageRelay`, GUID с перевёрнутыми байтами | меню грузится, проб нет (скрипт не найден) |
| меню, 3-й | то же с правильным GUID | падение `0x00480E03`: в меню нет состояния Lua |
| уровень, 1-й | три скрипта и объекты в `gf2/globals_global`, GUID с перевёрнутыми байтами | уровень грузится, проб нет |
| уровень, 2-й | то же с правильным GUID | 200+ проб: запуск при загрузке и по сообщению, приём сообщений, переменные, таймеры |
| уровень, 3-й | граф `examples/scripts/doctor_e2e.json`, размещённый `script-deploy`, + объекты | все пробы графа; `VariableSwitch`, `MessageRelay`, `VariableOperator` отвечают скрипту |

Что из этого следует:

- Свои скрипты работают в уровне, но не в меню.
- Порядок байтов GUID решает, найдёт ли игра скрипт.
- Сообщения скрипта, посланные во время загрузки, теряются.
- Игровое время на программном рендере идёт в ~20 раз медленнее настоящего: таймер на 10 с игрового
  времени срабатывает раз в 3–4 минуты.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §14
- [Скрипты](../../scripts.md) §1
- [Формат Lua](../../formats/lua.md)
- [Изменённый скрипт Lua](lua-modified-script.md)
