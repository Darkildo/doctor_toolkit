---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 49, 16]
sources: ["docs/analysis/live-link.md §5", "docs/analysis/live-link.md §11.3", "docs/logic-graph.md §1", "docs/logic-graph.md §9", "docs/formats/mission-logic.md §2.1", "research/ghidra/README.md", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, messages, bus]
updated: 2026-11-16
---
# Шина сообщений: имена, подписка, отправка

Адреса — виртуальные, издание PC 1.0.789.0. Сообщения движка — хэши `sdbm(lower(имя))` в глобальной таблице на 4096 корзин. «Ручка» сообщения создаётся из строки, подписка идёт через ту же таблицу, отправка синхронная.

| Адрес | Что это | Источник |
|---|---|---|
| `0x004DAFD0` | хэш движка `sdbm(lower(имя))`: `h = h*65599 + tolower(c)` | logic-graph.md §1; mission-logic.md §2.1 |
| `0x01162380` | данные: таблица сообщений (указатель на 4096 корзин по `хэш & 0xFFF`); запись: `[0]` следующая, `[1]` блоки слушателей, `[4]` хэш | live-link.md §5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00408260` | ищет хэш в таблице сообщений, создаёт запись, кладёт хэш в «ручку» объекта и увеличивает счётчики | logic-graph.md §1 |
| `0x00408240` | ручка из строки-литерала в коде движка (`iMsgChaseStarted`, …) | logic-graph.md §1 |
| `0x00408980` | ручка из строки: хэш + запись в таблицу, без подписки | logic-graph.md §1 |
| `0x004089B0` | то же и подписка объекта-получателя (`0x00407E60` → `0x004084D0`) | logic-graph.md §1 |
| `0x00407E60`, `0x004084D0` | подписка получателя на сообщение | logic-graph.md §1 |
| `0x00473EC0` | ручка и подписка с типом данных сообщения (`"BasePtrMsgData*"`) | logic-graph.md §1 |
| `0x00408A00` | отправка `bool Send(Message*, bool force)`, `cdecl`: обход подписчиков записи и вызов их обработчика (vtable `+4`); сообщение `{u32 хэш, u32 данные, u8 отправлено}`; 688 мест вызова (в базе Ghidra — 675 ссылок из 482 функций) | live-link.md §5; live-link.md §11.3; logic-graph.md §1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00408A05` | продолжение `0x00408A00` после пяти байт, занятых переходом модуля | live-link.md §11.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00408A70` | обход слушателей шиной: флаги узла `+0x14`, слушатель `+0xC`, включён — бит 0 в `+4` | live-link.md §6; crates/doctor-toolkit-live/src/edition.rs |
| `0x00408A7B` | место, где шина проверяет бит «слушатель включён» | crates/doctor-toolkit-live/src/addr.rs |
| `0x00408B70` | адресная отправка сообщения (из `0x004404F0`) | logic-graph.md §9 |
| `0x004404F0` | отправка с задержкой: задержка ≤ 0 — сразу, иначе в очередь на «сейчас + задержка × 1000 мс» игрового времени | logic-graph.md §9 |
| `0x00408840` | отписка слушателя (реагирует и отписывается) | logic-graph.md §9 |
| `0x00408900` | подписка точки появления на сообщение появления игрока | launch-flags.md §3.2 |
| `0x004C82C0` | Lua `SendMessage`: строит сообщение `{хэш, данные, флаг}` и зовёт `0x00408A00` | live-link.md §5; crates/doctor-toolkit-live/src/edition.rs |
| `0x0110064C` | данные: сообщение `iMsgPostSpawnInitialize`, которое движок шлёт объектам загруженной SimGroup | logic-graph.md §9 |
| `0x00445530` | задаёт имя сообщения `0x0110064C` | logic-graph.md §9 |
| `0x0040D640` | регистрация хэшей сообщений кадра (`iMsgBeforeRender`, `iMsgPreRender`, `iMsgDoRender`, …) | renderer.md §3 |
| `0x01206960` | данные: переменная с хэшем `iMsgBeforeRender` | live-link.md §12.2; crates/doctor-toolkit-live/src/addr.rs |

## Подробнее

- [Граф логики](../../logic-graph.md) таблица функций в начале, §9
- [Живая связь](../../analysis/live-link.md) §5, §11.3
- [Логика миссий](../../formats/mission-logic.md) §1
- [Заметки Ghidra](../../../research/ghidra/README.md) таблица функций
