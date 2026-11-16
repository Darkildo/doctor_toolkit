---
type: question
status: unknown
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 63, 77, 99]
sources: ["docs/analysis/queens.md §7", "docs/analysis/game-bench.md §30"]
tags: [question, crash, story]
updated: 2026-11-16
---
# Падение `0x6E756F7B` при входе в старт Миссии 3

Игра падает по адресу `0x6E756F7B` (вне образа) при входе в стартовый триггер Миссии 3, запущенной сообщением `cp_325_start_now` со старта `-stream`, — и без мода тоже. Быстрый путь к сюжету годится для чекпоинтов, но не для прохождения заданий. Причина в коде не разобрана.

## Следующий шаг

Снять стек и состояние в момент падения (перехват падений модулем), найти, какого сюжетного состояния не хватает при старте `-stream`.

Запланировано: [Этап 77: Сюжет и миссии](../research/stage-77.md), [Этап 99: Перехват падений](../research/stage-99.md).

## Связанное

- [Сюжет и миссии](../systems/story.md)
- [Сообщение `cp_325_start_now`](../messages/cp-325-start-now.md)
- [Известные падения: адреса](../exe/known-crashes.md)
- [Падения игры, найденные на стенде](../experiments/game-crashes.md)
- [docs/analysis/queens.md](../../analysis/queens.md) §7
