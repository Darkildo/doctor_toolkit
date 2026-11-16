---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 32]
sources: ["docs/analysis/game-bench.md §16", "docs/analysis/game-bench.md §27"]
tags: [launch-flags, cmd-txt, stream, playerspawn]
updated: 2026-11-16
---
# Быстрый вход без сохранения: -stream и -playerspawn

Этап 32. Игра читает флаги не из командной строки, а из `cmd.txt` в папке игры. `GAME_OPTIONS` стенда
пишет строку в `ТЕНЬ/cmd.txt` на прогон; `cmd.txt`, который стенд не писал, не трогается.

| `GAME_OPTIONS` | Прогонов | Падений | Управление |
|---|---|---|---|
| своя точка на террасе, `-stream gf2 cuba` | 2 | 0 | 20 с (нагрузка 1,5); `pos` — (179,407; 21,166; −25,438) |
| `-stream gf2 cuba` | 7 | 0 | 70–80 с (нагрузка 25–45) |
| `-stream gf2 miami` | 2 | 0 | ≤ 60 с |
| `-stream mp_mall mp_mall` | 5 | 0 | ≤ 65 с |
| `-stream gf2 queens` | 1 | 0 | ~100 с (нагрузка ~40) |
| два–четыре потока | 12 | 11 | — |
| `-stream gf2 cuba\local` | 3 | 3 | — |

Правила:

- `-stream` — последний флаг, имена без `.str`, один поток верхнего уровня: с двумя игра падает в
  `0x0042138B`, дочерний один — в `0x00890CEB`.
- `-stream gf2 cuba -windowed -resolution 1280 720` дважды упал в `0x0042138B` (этап 58): все слова
  после папки — список потоков. `-windowed -resolution 1280 720 -stream gf2 cuba` — работает.
- Точка `-playerspawn {GUID}` — из загружаемого потока (точка `gf2/cuba/local` при `-stream gf2 cuba`
  оставляет игрока в пустоте). Своя точка — копия точки по умолчанию (`level duplicate … gf2/cuba 6 1`),
  GUID — из `play … --list-spawns`.
- Сохранения и профиль не нужны; сюжет — как у новой игры.
- Сквозная проверка: `play --stream gf2/cuba` через `play-command.sh` — игра в Гаване, после выхода
  `took away cmd.txt`.

Следствие, найденное позже: после такого старта игрок не ходит без исправления ввода —
[игрок не ходит после `-stream`](player-not-walking-after-stream.md).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §16, §27
- [Флаги запуска](../../analysis/launch-flags.md) §2, §3
- [Падения игры на стенде](game-crashes.md)
