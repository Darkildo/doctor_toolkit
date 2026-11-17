---
type: exe
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 38, 16, 48, 77, 79]
sources: ["docs/logic-graph.md §9", "docs/analysis/story.md §2", "research/class_handlers.py", "docs/logic-graph.md §2", "docs/formats/mission-logic.md §4.1", "docs/analysis/live-link.md §13.7", "godfather2.exe 0x0047A1C0, 0x0047A320", "godfather2.exe: начала функций и слоты таблиц методов (сверка адресов)", "docs/analysis/class-behavior.md §2", "data/gf2/class_behavior.toml"]
tags: [exe, logic, simgroup, handlers]
updated: 2026-11-16
---
# Обработчики логических классов SimGroup

Адреса — виртуальные, издание PC 1.0.789.0. Обработчики сообщений и читатели атрибутов логических классов, по которым построен прогон графа логики. Часть режимов помечена в источнике как неразобранная («чёрный ящик»).

| Адрес | Что это | Источник |
|---|---|---|
| `0x0047A1C0` | обработчик `MultiManager` (слот `+4` таблицы слушателя `0x00E3306C`): по `trigger` шлёт пары `send_N`/`delay_N` | logic-graph.md §9 |
| `0x0047A320` | читатель атрибутов `MultiManager` (слот `0x00E33054` таблицы методов; разбирает пакет `0x6DF50074`) | logic-graph.md §9; mission-logic.md §4.1 |
| `0x004CF8D0`, `0x004CF950` | обработчик `MessageRelay` (слот таблицы слушателя — переходник `0x0047AA70` → `0x004CF8D0` → `0x004CF950`) | logic-graph.md §9; class-behavior.md §4 |
| `0x00479B60` | обработчик `AndEventGate` | logic-graph.md §9 |
| `0x00479E30` | обработчик `OrEventGate` | logic-graph.md §9 |
| `0x00477EF0` | обработчик `TriggerAuto` (по `iMsgPostSpawnInitialize`) | logic-graph.md §9 |
| `0x00477DB0` | конструктор `TriggerAuto`: в нём подписка на `iMsgPostSpawnInitialize` с приоритетом `0x7FFF` (прежний адрес `0x00477DA0` — переходник деструктора другого класса) | class-behavior.md §4 |
| `0x00900B80` | обновление `EnterExitTrigger` (список обновления `0x19`): вход, выход, повтор «внутри» | class-behavior.md §4 |
| `0x004C1550`, `0x004C1E40` | читатель атрибутов `TriggerBase` и путь срабатывания (интервал, скорость, направление) | class-behavior.md §4 |
| `0x00446BF0` | решает, создавать ли объект: флаги создания (атрибут 0 пакета `Base`, в записи объекта — байт `0x1D`) и фильтры хранилища состояния | class-behavior.md §2 |
| `0x0043D650` | фильтр создания хранилища состояния: объект с меткой `0x137726B7` не создаётся | class-behavior.md §2 |
| `0x0047AF60` | обработчик `TriggerRandom` | logic-graph.md §9 |
| `0x00476980`, `0x004768D0` | обработчик `VariableOperator` (`0x00476980` → `0x004768D0`) | logic-graph.md §9 |
| `0x004768D0` | запись переменной в `VariableOperator` (как Lua `VariableSet`) | live-link.md §13.7 |
| `0x00476E50` | обработчик `VariableSwitch` | logic-graph.md §9 |
| `0x00476FC0` | читатель атрибутов `VariableSwitch` (29 атрибутов) | logic-graph.md §2; mission-logic.md §4.1 |
| `0x00477B80`, `0x00477A90`, `0x00477BE0` | обработчики `VariableWatcher` | logic-graph.md §9 |
| `0x0090EA70` | обработчик `MissionTimer` | analysis/story.md §2, §3.6 |
| `0x0090E570` | читатель атрибутов `MissionTimer` | analysis/story.md §2 |
| `0x0090E2D0` | таймер интерфейса `MissionTimer` (атрибут 6): начало, длительность, флаги, подпись | analysis/story.md §3.6 |
| `0x0090D790` | `MissionTimer` с таймером на экране (не прочитано) | logic-graph.md §9 |
| `0x008FF870`, `0x008FF7E0` | обработчик `TestCurrentCheckpoint` (`0x008FF870` → `0x008FF7E0`); читатель — `0x008FF660` | analysis/story.md §2, §5 |
| `0x004C1260` | обработчик триггеров с пакетом `TriggerBase` (`TriggerBox`…) | logic-graph.md §9 |
| `0x00900E20` | обработчик `EnterExitTrigger`; читатель — `0x00900FF0` | analysis/story.md §2, §3.6 |
| `0x007279B0` | обработчик `InterestingObject`: сообщение 1 снимает объект с учёта, 2 возвращает | logic-graph.md §2; mission-logic.md §4.1 |
| `0x00727F30`, `0x00728070` | снять объект с учёта и поставить обратно (из `0x007279B0`) | logic-graph.md §2 |
| `0x009A6B00` | читатель атрибутов `NodeController` | logic-graph.md §2 |
| `0x00812980` | читатель атрибутов `EnableDisableStreams` | logic-graph.md §2 |

## Что неизвестно

Этап 79 прочитал код 45 классов и 5 общих пакетов: адреса конструктора, читателя и обработчика каждого, реакции и атрибуты — в `data/gf2/class_behavior.toml` и в заметках классов (раздел «Поведение»); адреса сверены с таблицами методов (`research/class_behavior.py tables`). Режимы `MessageRelay`, `trigger_2` у `VariableOperator`, атрибут 3 у `EnterExitTrigger` и начальное состояние триггеров теперь известны — [Поведение классов SimGroup](../../analysis/class-behavior.md) §4. Не известно: как часто физика сообщает о касании объёма ([вопрос](../questions/trigger-touch-frequency.md)), когда записи хранилища состояния попадают в сохранение ([вопрос](../questions/state-store-records.md)).

## Подробнее

- [Граф логики](../../logic-graph.md) §9, таблица обработчиков
- [Логика миссий](../../formats/mission-logic.md) §2
- [Вопрос: обработчики `MissionTimer` и `EnterExitTrigger`](../questions/missiontimer-enterexittrigger-handlers.md) — решён этапом 77: адреса взяты из таблиц методов
- [Классы сюжета: читатели и обработчики](story-classes.md)
