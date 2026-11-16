---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 34]
sources: ["docs/analysis/game-bench.md §18"]
tags: [live-link, module, teleport, lua, messages]
updated: 2026-11-16
---
# Живая связь: первый прогон модуля в игре

Этап 34. Тень со всеми файлами игры, модуль — `doctor-toolkit live install ТЕНЬ --writes`, сеанс
`start` с `WINEDLLOVERRIDES=dinput8=n,b`, `GAME_OPTIONS="-stream gf2 cuba"`, `MEMORY=1`.

| Проверка | Результат |
|---|---|
| модуль грузится | журнал: `edition PC 1.0.789.0, frame hook at 0x0068B04B, update hook at 0x00D57754: true`, `listening on 127.0.0.1:47811` |
| координаты | модуль (283,897; −0,000; 371,797), курс −109,4°; `gamemem.py` — то же до миллиметра (50 матриц) |
| камера | (288,196; 1,742; 372,961), 4,5 м за игроком; угол обзора 36,0° (1024×768) |
| время, потоки, объекты | время суток идёт (16:27:30 → 16:31:00 за 20 с); 61 поток; обход 8 639 объектов — 2 мс |
| телепорт | (283,90; −0,01; 371,80) → (274,500; −0,050; 368,500) через 5–10 с, игрок опустился на землю |
| сообщение | `send boundary_fade_out` — 1 слушатель (`FadeScreenFx`): средняя яркость снимка 85,7 → 1,2 |
| Lua | `return GetSimTime(), 1+2, "hi", nil, GetPlatform()` → `userdata…`, 3, hi, nil, windows; ошибки возвращаются текстом |
| 10 минут | 603 с с модулем и GF2ASI, телепорт трижды — падений нет |
| «Играть» и откат | `play … --stream gf2/cuba --live --live-writes`: после выхода `took away cmd.txt`, `took away the live module`; 1381 файл тени побайтно как до запуска |

Найдено:

- **Запись вне кадра роняет игру.** Телепорт из перехвата `0x0068B04B` — `Unhandled page fault … at
  address 0040C278`: затемнение пишет в буфер команд кадра `0x01206880`, который вне кадра пуст.
  Запись перенесена в обновление `GameMainLoop` (слот `+0x2C` таблицы `0x00D57728`).
- **`VirtualQuery` под Wine медленный.** Обход объектов с проверкой каждого чтения держал главный
  поток больше 3 минут; теперь — карта регионов на задание и бюджет 400 мс.
- Без `WINEDLLOVERRIDES=dinput8=n,b` Wine грузит свой `dinput8.dll`, и ASI-загрузчик не работает.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §18
- [Живая связь](../../analysis/live-link.md) §1–§9
