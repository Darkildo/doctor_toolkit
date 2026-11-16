---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 49]
sources: ["docs/analysis/game-bench.md §21", "docs/analysis/live-link.md §11.6"]
tags: [live-link, journal, lua, messages, objects]
updated: 2026-11-16
---
# Журнал игры и все объекты (этап 49)

Быстрый стенд: `-stream gf2 cuba`, `DXVK=1`, `SERVER=xvfb`, модуль `live install --writes` (в
`doctor_live.cfg` по умолчанию `journal=1`).

| Проверка | Результат |
|---|---|
| перехваты | журнал модуля: `OutputDebugStringA (0x00D550C4), Lua print (0x0110A6C0, was 0x0046bfc0), Lua errors (0x006365A0, 0x00636722, 0x00636488, stacks 0x006338A0), message bus (0x00408A00)` |
| вывод игры | `print Global Lua Utility Functions:19: functionTable[startup] is nil` (дважды при загрузке); `live lua 'print("hello from doctor", 42)'` попадает в журнал. Сама игра в уровне `OutputDebugStringA` за 11 минут не звала |
| ошибки Lua | синтаксис, `error("boom")` со стеком, `nil.y` в третьей вложенной функции — три уровня со строками 1, 2, 3 |
| сообщения | каждый кадр — 15 сообщений движка (`iMsgRunningTick data 0x672 -> 343 listener(s)`, `iMsgRunningPostTick -> 1133`…); игровые — `setExpression_Happy`, `aud_test_tod`, `aud_night`…; за 10 минут — 224 разных сообщения |
| все объекты | 8 664 объекта за 7 мс, 7 030 без матрицы; `SimNpc` 3 418, `ZoneRenderLOD` 475, `SpawnPoint` 172…; `gf2\globals_global.str` — 6 080 |
| поиск | по GUID — 0,45 с; по имени `venue_132_vault` — два `Entity` в `gf2\cuba\local.str` |
| 10 минут | 610 с: 55 из 55 ответов статуса, `live log --follow --messages` (8 984 строки), 18 раз все объекты; падений нет; `verify`: оригиналы не менялись (27 файлов) |

Найдено:

- **Вызов `As` у всех объектов вешает игру**: у части объектов таблицы групп в слоте `+0x10` не `As`
  (`0xFFFFFFFF`, адрес вне кода). Класс теперь читается из кода `As` без вызова.
- Указатель печати Lua `0x0110A6C0` не пуст: в нём пустая функция `0x0046BFC0`; модуль зовёт её
  после своей.
- Без предела на одно сообщение такты движка вытесняли бы всё остальное (2 500+ событий за 5 с).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §21
- [Живая связь](../../analysis/live-link.md) §11
