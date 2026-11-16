---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 21, 24]
sources: ["docs/analysis/game-bench.md §13.7"]
tags: [play, workspace, revert]
updated: 2026-11-16
---
# «Играть» с записью и откатом на стенде

Этап 24, проверка этапа 21. Способ «своя команда» запускает стенд вместо Wine пользователя:
`doctor-toolkit play ТЕНЬ --method command --command 'tools/bench/play-command.sh {dir}' --apply
--revert-after`. В рабочем пространстве тени — новая строка `doctor_play` («PLAY TEST: APPLIED BY
DOCTOR-TOOLKIT») и ссылка `$fe_press_start` → `$doctor_play` в `frontend.apx`, не применены.

| Шаг | Вывод `play` | Проверка |
|---|---|---|
| запись | `applied: 5 stream(s) written, 17 file(s)` | — |
| запуск | `started`, `the game is running (process …)` | на титульном экране «PLAY TEST: APPLIED BY DOCTOR-TOOLKIT» |
| выход | `the game has exited` | стенд остановил игру на 75-й секунде |
| откат | `reverted: 5 stream(s) put back, 17 file(s)` | 5 правок снова `pending`; пять потоков и TOC побайтно равны файлам игры |

Весь цикл — 6 мин 36 с, из них ~5 мин `play` ждал: слежение за процессом видит любой `godfather2.exe`
на машине, а другой агент запускал свою копию игры. Для одного пользователя это верно (вторая копия
держит те же файлы), для стенда рядом с другими — нужно учитывать
(`DOCTOR_TOOLKIT_GAME_PROCESS`, см. [проблемы стенда](bench-problems.md)).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §13.7
- [Руководство: «Играть»](../../guide/11-play.md)
- [Рабочее пространство](../../workspace.md)
