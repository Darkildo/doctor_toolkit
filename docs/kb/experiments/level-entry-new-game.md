---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 14]
sources: ["docs/analysis/game-bench.md §6", "docs/analysis/game-bench.md §6.1"]
tags: [bench, new-game, timing, streams]
updated: 2026-11-16
---
# Вход в уровень через новую игру: путь и время

Сценарии `tools/bench/new-profile.txt` и `tools/bench/new-game.txt` ведут игру до первого управления
в уровне (терраса отеля в Гаване). Префикс стенда, 1024×768, llvmpipe.

Путь:

1. Заставки Paramount и EA не пропускаются; титульный экран — на 48–55-й секунде.
2. «Press any key» → `Return`; «Select Profile»; окно «Autosave is now turned on» → `Return`.
3. Главное меню принимает только клавиши: `seek new-game-selected Down`, затем `Return`.
4. Ролик-титры (не пропускается), экран внешности персонажа (`Tab`; `seek !character Tab`).
5. Вступительная сцена на движке (сегменты `igc01_s01…s15`) не пропускается.
6. Управление: подсказка «TALK: Press E…», `$0` и полоска здоровья (эталон `hud`). Ходьба —
   `hold w/a/s/d`; относительное движение мыши (`look`) камеру на стенде не поворачивает.

Время (нагрузка машины 9–25): титульный экран 48 с, главное меню 53 с, экран внешности 201 с,
управление 845 с. Вступительная сцена идёт ~10 минут вместо ~3: 1,4 кадра/с, а сцена идёт по кадрам.
Сохранения на этом месте игра сама не делает (за 4 минуты на террасе не появилось ни одного файла).

`TRACE_FILES=1` показал 86 потоков к появлению управления: глобальные (`globals_global`, `cuba*`,
`miami*`, `queens*`, `allcharacters`), `c01_001_folderstream`, `cp_009_hotel_interior_folderstream` и
его `igc01_s*`, `gf2/cuba/local` и зоны района `xenia`, `sky`, `ocean`.

Что из этого следует: прогон через новую игру — 13–17 минут; для частых проверок нужны
[сохранение после пролога](save-after-prologue.md) или [старт `-stream`](quick-entry-stream-flag.md).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §6, §6.1
- [Устройство стенда](bench-setup.md)
