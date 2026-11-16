---
type: research
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72]
sources: ["docs/kb/README.md", "crates/doctor-toolkit/src/kb/mod.rs"]
tags: [research, knowledge-base]
updated: 2026-11-16
---
# Этап 72: База знаний (Obsidian)

- Состояние: сделан (2026-11-16).
- Зависит от: —.
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- `docs/` — хранилище Obsidian (`docs/.obsidian/`: без плагинов сообщества, цвета графа по статусу и типу).
- `docs/kb/` — эта база: [карта](../index.md), карты разделов, заметки движка, систем игры, классов, типов ресурсов, сообщений, адресов exe, вопросов, опытов и этапов исследований.
- Генератор `doctor-toolkit kb-gen` (классы, типы ресурсов, сообщения, списки папок) и проверка `doctor-toolkit kb-lint` (в тестах и CI).
- MCP: ресурсы `doctor-toolkit://kb/…`, инструменты `kb_search` и `kb_note`.

## Дальше

Каждый следующий этап исследований обновляет базу: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
