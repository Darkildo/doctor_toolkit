---
type: map
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72, 102]
sources: []
tags: [map, tools]
updated: 2026-11-16
---
# Карта: инструменты

Чем пользоваться, чтобы смотреть, менять и проверять игру. Здесь только указатели: сами инструменты описаны в документах `docs/`. Назад: [карта базы](../index.md).

## Тулкит

- [Руководство пользователя](../../guide/README.md) — редактор по шагам.
- [Командная строка и оболочка](../../shell.md), [поиск и навигация по игре](../../navigation.md), [просмотрщики](../../viewers.md).
- [Рабочее пространство](../../workspace.md) — правки с историей и откатом; [проект мода](../../project.md); [моды и менеджер модов](../../mods.md).
- [Уровни](../../levels.md), [граф логики](../../logic-graph.md), [скрипты](../../scripts.md).
- Модели: [форматы моделей](../../model-formats.md), [импорт моделей](../../model-import.md), [Blender](../../blender.md), [внешние редакторы](../../external-editors.md).
- [Привязки Python](../../python.md).
- [Что где доступно](../../parity.md) — каждая возможность тулкита по поверхностям: MCP, редактор, командная строка, Python; заметка: [инвентаризация поверхностей](../research/stage-102.md).

## Для агентов

- [MCP-сервер](../../mcp.md) — игра и база знаний через Model Context Protocol: ресурсы `doctor-toolkit://kb/…`, инструменты `kb_search` и `kb_note`.
- [README базы знаний](../README.md) — как читать и как обновлять базу, шаблоны заметок, `kb-gen` и `kb-lint`.

## Игра вживую

- [Стенд проверки в игре](../../analysis/game-bench.md) — заметка: [устройство стенда](../experiments/bench-setup.md).
- [Живая связь](../../analysis/live-link.md) — заметка: [модуль `doctor_live`](../engine/live-module.md).
- [Флаги запуска](../../analysis/launch-flags.md) — заметка: [флаги запуска и `cmd.txt`](../engine/launch-flags.md).
- Графика и скорость: [рендер](../../analysis/renderer.md), [производительность игры](../../analysis/performance.md), [производительность редактора](../../analysis/editor-performance.md).

## Исследование

- Скрипты по корпусу — папка `research/`; заметки о Ghidra — [research/ghidra](../../../research/ghidra/README.md).
- Данные схем — `data/gf2/`: имена классов, виды атрибутов, пины, сообщения движка, типы ресурсов, параметры игры. Из них собираются заметки [классов](../classes/index.md), [типов ресурсов](../resources/index.md) и [сообщений](../messages/engine-messages.md).

## Известные ограничения инструментов

[Редактор уровней](../questions/level-viewer-limits.md), [индекс ссылок](../questions/unextracted-references.md), [менеджер модов](../questions/mods-manager-open-items.md), [проект мода](../questions/project-open-items.md), [Python](../questions/python-open-items.md), [видео](../questions/video-open-items.md), [запись metamodel](../questions/metamodel-full-encode.md), [динамические тела](../questions/dynamic-bodies.md), [сопрограммы в ВМ Lua](../questions/lua-coroutines-in-toolkit-vm.md), [порядок пакетов](../questions/packet-order-after-attribute-removal.md).
