---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 38, 77, 79]
sources: ["docs/analysis/story.md §2, §3.6", "research/class_handlers.py", "godfather2.exe 0x0090E570, 0x0090EA70, 0x00900FF0, 0x00900E20", "docs/logic-graph.md §9.2", "godfather2.exe 0x0090DF00, 0x00900A70, 0x0090DA30", "data/gf2/class_behavior.toml"]
tags: [question, exe, logic, simgroup, handlers]
updated: 2026-11-16
---
# Где обработчики сообщений `MissionTimer` и `EnterExitTrigger`?

Решено этапом 77. Адреса найдены по таблицам методов, без догадок: читатель класса лежит в слоте 1 таблицы объекта, конструктор кладёт таблицу слушателя в `[объект+0x3C]`, обработчик — её слот 1 (`research/class_handlers.py`; для `AndEventGate` способ даёт известный `0x00479B60`).

## Ответ

| Класс | Читатель | Таблица объекта | Таблица слушателя | Обработчик |
|---|---|---|---|---|
| `MissionTimer` | `0x0090E570` | `0x00D83770` | `0x00D83760` | `0x0090EA70` |
| `EnterExitTrigger` | `0x00900FF0` | `0x00D817E8` | `0x00D817D8` | `0x00900E20` |

Прежние адреса `0x0090DF30` и `0x00900B30` лежат внутри других функций; `0x0090DA30` — не читатель `MissionTimer`.

Поведение по новым адресам перечитано: у `MissionTimer` описание в целом подтвердилось (старт, такт, `on_expired`, `stop` → `on_stopped`, пауза и продолжение) и дополнено таймером интерфейса; у `EnterExitTrigger` выключение отложено до такта `iMsgRunningPostTick`. Подробно — [story.md](../../analysis/story.md) §3.6.

Этап 79 подтвердил и дополнил: `EnterExitTrigger`: обработчик — `0x00900E20`, слот 1 таблицы слушателя `0x00D817D8`, которую пишет конструктор `0x008FFC50` (сверено чтением таблицы из образа и в запущенной игре — [опыт](../experiments/class-state-in-game.md)); читатель — `0x00900FF0`. `0x00900B30` лежит в деструкторе `0x00900A70`. Вход, выход и повтор «внутри» считает обновление `0x00900B80` (список обновления `0x19`). `MissionTimer` разбирает этап 77.

## Что осталось

Ничего: выходы `EnterExitTrigger` различены по коду этапом 79 — 0 «вошёл», 1 «вышел», 2 «внутри» с периодом атрибута 3 (обновление `0x00900B80`; `docs/analysis/class-behavior.md`).

## Связанное

- [Обработчики логических классов SimGroup](../exe/logic-class-handlers.md)
- [Сюжет и миссии](../systems/story.md)
- [docs/logic-graph.md](../../logic-graph.md) §9.2
