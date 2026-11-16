---
type: message
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 60, 63]
sources: ["docs/analysis/game-bench.md §30", "docs/analysis/queens.md §7"]
tags: [messages, story, checkpoints]
updated: 2026-11-16
---
# Сообщение `cp_325_start_now`

Сообщение чекпоинта сюжета. Посланное вживую (`live send cp_325_start_now`) со старта `-stream`, включает Миссию 3 «Укрепление Кармине»: текущий чекпоинт становится `cp_325`, идут экраны «Compound UNLOCKED», древо семьи, «Взгляд дона».

Ограничение: вход в стартовый триггер миссии у (232; 14; −345) после такого запуска роняет игру по адресу `0x6E756F7B` — и без мода тоже. Быстрый путь годится для чекпоинтов, но не для прохождения заданий.

## Связанное

- [Сюжет и миссии](../systems/story.md)
- [Падение `0x6E756F7B` при входе в старт Миссии 3](../questions/crash-6e756f7b.md)
- [Мод «Новый Нью-Йорк» для игроков: менеджер модов, босс, машины (этап 63)](../experiments/new-new-york-players.md)
- [Чекпоинты миссий и переменные сюжета](../exe/missions-variables.md)
- [docs/analysis/queens.md](../../analysis/queens.md) §7
