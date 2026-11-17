---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [99]
sources: ["docs/analysis/crash-reports.md", "docs/analysis/game-bench.md §37", "crates/doctor-toolkit-live/src/crash.rs", "data/gf2/exe_symbols.tsv"]
tags: [research, crashes]
updated: 2026-11-16
---
# Этап 99: Перехват падений

- Состояние: сделан (2026-11-16).
- Зависит от: [этап 72](stage-72.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Отдельный модуль игры `doctor_crash.asi`: фильтр необработанных исключений, текстовый отчёт и минидамп в `_doctor_toolkit/crashes/`; работает без модуля связи и вместе с ним — [описание](../../analysis/crash-reports.md), [адреса](../exe/crash-handler.md).
- Имена мест в отчёте — из таблицы `data/gf2/exe_symbols.tsv`, которую `doctor-toolkit kb-gen` собирает из заметок [адресов `exe`](../exe/index.md).
- Редактор и менеджер модов показывают последнее падение и открывают отчёт; CLI `crash-report`.
- На стенде: настоящее падение `0x0042138B`, падение по заказу, работа вместе с модулем связи — [опыт](../experiments/crash-reports-bench.md).
- Известное падение `0x6E756F7B`: адрес читается как текст `{oun` — [вопрос](../questions/crash-6e756f7b.md).

## Что осталось

- Причина падения `0x6E756F7B` — [вопрос](../questions/crash-6e756f7b.md).
- Адресная доставка сообщений (`0x00408BF0`) и очередь отложенных в отчёт не попадают; получателей сообщений и стека Lua в отчёте нет.
- Не проверено: Windows, переполнение стека, зеркало Large Address Aware.

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
