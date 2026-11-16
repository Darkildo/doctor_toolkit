---
type: exe
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 38, 16, 48]
sources: ["docs/logic-graph.md §9", "docs/logic-graph.md §2", "docs/formats/mission-logic.md §4.1", "docs/analysis/live-link.md §13.7", "godfather2.exe 0x0047A1C0, 0x0047A320", "godfather2.exe: начала функций и слоты таблиц методов (сверка адресов)"]
tags: [exe, logic, simgroup, handlers]
updated: 2026-11-16
---
# Обработчики логических классов SimGroup

Адреса — виртуальные, издание PC 1.0.789.0. Обработчики сообщений и читатели атрибутов логических классов, по которым построен прогон графа логики. Часть режимов помечена в источнике как неразобранная («чёрный ящик»).

| Адрес | Что это | Источник |
|---|---|---|
| `0x0047A1C0` | обработчик `MultiManager` (слот `+4` таблицы слушателя `0x00E3306C`): по `trigger` шлёт пары `send_N`/`delay_N` | logic-graph.md §9 |
| `0x0047A320` | читатель атрибутов `MultiManager` (слот `0x00E33054` таблицы методов; разбирает пакет `0x6DF50074`) | logic-graph.md §9; mission-logic.md §4.1 |
| `0x004CF8D0`, `0x004CF950` | обработчик `MessageRelay` (`0x004CF8D0` → `0x004CF950`) | logic-graph.md §9 |
| `0x00479B60` | обработчик `AndEventGate` | logic-graph.md §9 |
| `0x00479E30` | обработчик `OrEventGate` | logic-graph.md §9 |
| `0x00477EF0` | обработчик `TriggerAuto` (по `iMsgPostSpawnInitialize`) | logic-graph.md §9 |
| `0x00477DA0` | подписка `TriggerAuto` | logic-graph.md §9 |
| `0x0047AF60` | обработчик `TriggerRandom` | logic-graph.md §9 |
| `0x00476980`, `0x004768D0` | обработчик `VariableOperator` (`0x00476980` → `0x004768D0`) | logic-graph.md §9 |
| `0x004768D0` | запись переменной в `VariableOperator` (как Lua `VariableSet`) | live-link.md §13.7 |
| `0x00476E50` | обработчик `VariableSwitch` | logic-graph.md §9 |
| `0x00476FC0` | читатель атрибутов `VariableSwitch` (29 атрибутов) | logic-graph.md §2; mission-logic.md §4.1 |
| `0x00477B80`, `0x00477A90`, `0x00477BE0` | обработчики `VariableWatcher` | logic-graph.md §9 |
| `0x0090DF30` | обработчик `MissionTimer` | logic-graph.md §9 |
| `0x0090DA30` | читатель атрибутов `MissionTimer` | logic-graph.md §9 |
| `0x0090D790` | `MissionTimer` с таймером на экране (не прочитано) | logic-graph.md §9 |
| `0x008FF590`, `0x008FF4F0` | обработчик `TestCurrentCheckpoint` (`0x008FF590` → `0x008FF4F0`) | logic-graph.md §9 |
| `0x004C1260` | обработчик триггеров с пакетом `TriggerBase` (`TriggerBox`…) | logic-graph.md §9 |
| `0x00900B30` | обработчик `EnterExitTrigger` | logic-graph.md §9 |
| `0x007279B0` | обработчик `InterestingObject`: сообщение 1 снимает объект с учёта, 2 возвращает | logic-graph.md §2; mission-logic.md §4.1 |
| `0x00727F30`, `0x00728070` | снять объект с учёта и поставить обратно (из `0x007279B0`) | logic-graph.md §2 |
| `0x009A6B00` | читатель атрибутов `NodeController` | logic-graph.md §2 |
| `0x00812980` | читатель атрибутов `EnableDisableStreams` | logic-graph.md §2 |

## Что неизвестно

Режимы `MessageRelay` с атрибутами 2 и 4, `trigger_2` у `VariableOperator`, задержка 3 у `EnterExitTrigger` не разобраны; начальное состояние триггеров не известно.

## Подробнее

- [Граф логики](../../logic-graph.md) §9, таблица обработчиков
- [Логика миссий](../../formats/mission-logic.md) §2
- [Вопрос: обработчики `MissionTimer` и `EnterExitTrigger`](../questions/missiontimer-enterexittrigger-handlers.md) — два адреса таблицы не являются началами функций
