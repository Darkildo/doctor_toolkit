---
type: question
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 73, 74, 78]
sources: ["docs/scripts.md §9", "docs/logic-graph.md §1", "data/gf2/messages.toml", "docs/analysis/lua-api.md §2", "docs/analysis/game-bench.md §35", "docs/analysis/message-bus.md §4", "data/gf2/bus_sites.tsv", "docs/analysis/game-bench.md §36", "godfather2.exe 0x0045CA00, 0x004C8510"]
tags: [question, lua, messages]
updated: 2026-11-16
---
# Что лежит в данных сообщения (`data`) и как читать их из Lua

Этап 74 выяснил сторону Lua. Обработчик получает `(self, имя, имяТипа, данные)`: `имяТипа` — то, что сам скрипт передал вторым аргументом `AddMessage` (движок его не сверяет), `данные` — light userdata, сырой указатель из сообщения, или `nil`. `SendMessage(имя, тип, данные)` шлёт указатель на число (`float`), на копию текста, на память userdata или сам light userdata; без строки `тип` данные не уходят. `SendMessageEx` кладёт до восьми значений в структуру `EARS::Framework::Lua::MsgData` (vtable `0xE365DC`) и возвращает то, что дописал слушатель. Читать память по указателю из Lua можно только `GetPositionValues` (три `float`), `GetImpactData` и передачей обратно в `SendMessageEx`. Всё проверено на стенде.

Ответ этапа 78 на первую половину: слово данных — 0 или указатель на объект данных, который отправитель собрал на стеке; см. [данные сообщений](../engine/message-data.md).

- Объекты SimGroup кладут указатель на объект (таблица `0x00E31E2C`: на себя или на вошедшего в триггер) либо пересылают полученное слово; у сообщений движка — 18 своих структур.
- Имя типа в подписке (`"BasePtrMsgData*"`, `0x00473EC0`) кодом не читается.
- В типах поле `data` заполнено у 310 из 645 имён движка (у 277 — для всех найденных отправителей) и у 33 023 из 42 087 сообщений файлов игры (у 32 890 — для всех отправителей); источник — места кода (`data/gf2/bus_sites.tsv`) и таблица `class_data` в `data/gf2/messages.toml`, уверенность `exe`.
- Lua `SendMessage(имя, второй, данные)` берёт данные из третьего аргумента: число → указатель на `f32`, строка → указатель на копию строки (`exe`, проверено на стенде).

Не установлено:

- чей объект в данных у `Checkpoint`, `StandardDamageComponent`, `GenericCSA`, `TeamCapturableCSA`; данные остальных классов-отправителей (11 082 сообщения каталога остаются `unknown`); У `InterestingObject` (персонаж-пользователь), `SpawnPoint` (появившийся персонаж) и `FuncSpawn` (созданный объект) выяснено этапом 79 — `data/gf2/messages.toml`, `class_data`.
- поля структуры урона (`0x00D6A7A8`) и точный смысл полей остальных структур.

## Следующий шаг

Для классов без записи в `class_data` прочитать место отправки (строки `[obj+…]` рядом с читателем класса в `data/gf2/bus_sites.tsv`) и откуда берётся объект; на стенде — `live log --messages` с протоколом 8 показывает объект в данных. Сторону Lua — разобрать вызов приёмника (`f(self, имя, nil, данные)`) и функции над `data`.

## Связанное

- [Данные сообщения](../engine/message-data.md)
- [Lua в игре](../engine/lua.md)
- [Шина сообщений](../engine/message-bus.md)
- [Шина сообщений: имена, подписка, отправка](../exe/message-bus.md)
- [docs/scripts.md](../../scripts.md) §9
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
- [Разбор шины сообщений](../../analysis/message-bus.md) §4
