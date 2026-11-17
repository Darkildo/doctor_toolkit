---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [80]
sources: ["docs/analysis/asi-platform.md", "docs/analysis/game-bench.md §43", "crates/doctor-toolkit-live/src/platform.rs", "crates/doctor-toolkit-sdk/include/doctor_sdk.h", "examples/native-class/build.sh", "research/bench_platform.sh"]
tags: [research, native-mods, classes, lua, hooks]
updated: 2026-11-16
---
# Этап 80: ASI-платформа: новые классы и Lua-функции

- Состояние: сделан (2026-11-16).
- Зависит от: [этап 74](stage-74.md), [этап 78](stage-78.md), [этап 79](stage-79.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Модуль `doctor_platform.asi`: классы SimGroup из DLL регистрируются функцией движка `0x004833A0`, игра создаёт их объекты сама; функции Lua — через связку LuaManager (`0x0043E2E0`); сообщения — функциями шины; перехваты модов — с проверкой по общему реестру — [описание](../../analysis/asi-platform.md), [заметка](../engine/native-mods.md), [адреса](../exe/native-mods.md).
- Реестр мест, которые патчат все модули тулкита (`platform::CLAIMS`, 52 места), и тесты пересечений; три старых модуля на общий API времени выполнения не переводились — [вопрос](../questions/modules-on-shared-hook-api.md).
- SDK: крейт `doctor-toolkit-sdk` и заголовок C `doctor_sdk.h`, ABI версии 1; пример `examples/native-class` (мод на Rust и мод на C, данные мода, одна команда сборки).
- Схема классов мода для тулкита: файл TOML, переменная `DOCTOR_TOOLKIT_CLASSES`, команды `class-schema` и `simgroup-add`.
- На стенде: объекты двух новых классов созданы игрой, читают атрибуты, слушают и шлют сообщения; функции Lua отвечают скрипту уровня; фильтр и перехват работают; выгрузка и загрузка потока с объектом, автосохранение и загрузка сохранения — без падений — [опыт](../experiments/native-class-on-bench.md).
- По корпусу: все 111 518 записей объектов игры упакованы (`research/record_modes.py`).

## Что осталось

- [Состояние объектов мода в сохранениях](../questions/native-class-saved-state.md).
- [Классы-сущности мира из DLL](../questions/native-entity-classes.md).
- [Перевод модулей на общий API перехватов](../questions/modules-on-shared-hook-api.md).
- Установка нативных модов менеджером — [этап 81](stage-81.md) (сделан).

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
