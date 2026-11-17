---
type: engine
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [78]
sources: ["docs/analysis/message-bus.md §4", "data/gf2/messages.toml", "data/gf2/bus_sites.tsv", "docs/analysis/game-bench.md §36", "godfather2.exe 0x0045CA00, 0x00476C80, 0x0047A1C0, 0x004C8510"]
tags: [message-bus, data, simgroup]
updated: 2026-11-16
---
# Данные сообщения

Второе слово сообщения [шины](message-bus.md). Оно равно 0 либо указывает на объект данных, который отправитель собрал у себя на стеке.

## Что известно

- Объект данных живёт только на время рассылки; в [очередь задержки](message-delivery.md) кладётся копия в куче. Первое слово — таблица методов с одним методом (деструктор). Имён этих классов и RTTI в exe нет; получатель знает раскладку по сообщению.
- Имя типа в подписке (`"BasePtrMsgData*"` у `0x00473EC0`) функцией не читается — это пометка исходников, а не проверка.
- **Указатель на объект**, таблица `0x00E31E2C`: `{таблица, объект + 0x48, следующий}` — звено списка безопасных указателей объекта. Конструктор `0x0045CA00`. Объект — `[данные + 4] − 0x48`.
- Шлюзы и автотриггеры кладут указатель на **себя**: `TriggerAuto`, `AndEventGate`, `OrEventGate` (у `OrEventGate` проверено на стенде).
- Реле **пересылают** слово данных пришедшего сообщения: `MultiManager`, `TriggerRandom`, `VariableSwitch`, `TestTimeOfDay`. Объём (`TriggerBase`) пересылает данные `iMsgTrigger`, а в них — **кто вошёл**. Поэтому в конце цепочки в данных лежит тот, кто её начал.
- Без данных шлют: `PhoneMessage`, `ScriptedSequence`, `TestBuildingState`, `TestUpgrade`, `TriggerDistance`, `TriggerFollow`, `SimpleChatterBox`, ролики, `score`.
- У сообщений движка — свои структуры (здание, раздражитель ИИ, событие счёта, взрыв…): 18 таблиц методов, ключи и поля — в [сообщениях файлов игры](../messages/game-messages.md) (`data_type` в `data/gf2/messages.toml`).
- Lua `SendMessage(имя, второй, данные)`: число → указатель на `f32`, строка → указатель на копию строки, userdata → тело; данные берутся из третьего аргумента (`exe`, `bench`). `SendMessageEx` кладёт объект `0x00E365DC` с парами «значение, тип».
- В типах тулкита слово данных известно у 310 имён движка и у 33 023 из 42 087 сообщений файлов игры; [модуль связи](live-module.md) пишет в журнал GUID объекта из данных.

## Что неизвестно

- Чей объект кладут `InterestingObject`, `Checkpoint`, `SpawnPoint`, `StandardDamageComponent`, `GenericCSA`; поля структуры урона; как приёмник Lua читает `data` — [вопрос](../questions/lua-message-data.md).

## Подробнее

- [Разбор шины сообщений](../../analysis/message-bus.md) §4
