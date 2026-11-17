---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [77]
sources: ["docs/analysis/story.md", "docs/analysis/game-bench.md §40", "examples/quests/new-quest.sh", "research/mission_survey.py", "research/class_handlers.py", "crates/doctor-toolkit/src/path_limit.rs"]
tags: [research, story, missions]
updated: 2026-11-16
---
# Этап 77: Сюжет и миссии

- Состояние: сделан (2026-11-16).
- Зависит от: [этап 74](stage-74.md), [этап 76](stage-76.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Список миссий: 69 глав, 166 чекпоинтов — все объекты `Checkpoint` игры; жизнь чекпоинта, перезапуск, провал, запись в сохранение — [сюжет и миссии](../systems/story.md), [разбор](../../analysis/story.md).
- Цели и интерфейс: `UIHudObjective`, текущая цель во «Взгляде дона», услуги `UIJobInfo`; награды `JobReward`, `ExecuteScoreEvent`, `UIFanfare` — [адреса](../exe/story-classes.md).
- Верные адреса обработчиков `MissionTimer`, `EnterExitTrigger` и `TestCurrentCheckpoint` — [вопрос решён](../questions/missiontimer-enterexittrigger-handlers.md).
- Падение `0x6E756F7B`: причина — длина пути папки игры, буфер 256 байт в чтении файлов; правило и предупреждения в тулките — [вопрос решён](../questions/crash-6e756f7b.md), [очередь чтения файлов](../exe/async-file-io.md).
- Пример своего задания `examples/quests/new-quest.sh` и его проверка в игре — [опыт](../experiments/story-bench.md).
- В тулките: `logic-import` добавляет новые переменные в словарь; отчёт о падении печатает память по регистрам; `validate`, `play` и стенд предупреждают о длинной папке игры.

## Что осталось

- [Открытые вопросы сюжета и заданий](../questions/story-open-items.md): загрузка сохранения с новым чекпоинтом, перезапуск после провала, «History Log», база очков, `HitMission` и `DynamicJob`.

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
