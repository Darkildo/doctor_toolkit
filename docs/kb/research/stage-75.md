---
type: research
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [75]
sources: ["docs/analysis/handing-out.md", "docs/analysis/game-bench.md §39", "research/exe_dis.py", "research/dialogue_exe.py", "research/dialogue_vars.py", "research/items_survey.py", "research/items_find.py", "examples/trader/build.sh"]
tags: [research, items, money, weapons, dialogues]
updated: 2026-11-16
---
# Этап 75: Предметы, деньги, оружие из логики; выбор как меню

- Состояние: сделан (2026-11-16) по коду exe, файлам игры и стенду; выбор в меню клавишей на стенде не получился.
- Зависит от: [этап 74](stage-74.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- **Выдача**: [GiveItemRelay](../classes/GiveItemRelay.md) и предок [AttributeRelay](../classes/AttributeRelay.md) прочитаны в коде — флаги, количество, три способа задать цель; оружие, патроны и уровень оружия проверены на стенде ([оружие](../systems/weapons.md)).
- **Деньги**: [MoneyAdaptor](../classes/MoneyAdaptor.md) начисляет и списывает, но только от сообщения с персонажем в данных; предмет «Cash» начисляет; «счёт» — предмет с сообщением подбора — кладёт игрока в данные любого сообщения ([экономика](../systems/economy.md)). Категории книг названы по таблице статистики.
- **Предметы**: читатель `Item`, виды и 11 слотов инвентаря; нового вида оружия завести нельзя, новое оружие — новый объект `Gun` существующего вида.
- **Улучшения**: уровень оружия выдаётся реле; навык `UpgradeData` из логики одиночной игры выдать нечем.
- **Диалог как меню**: «переменные» — номера запросов; `0x23`, `0x27`, `0x24` шлют три сообщения `ChangeNPCSettings`; новый диалог — свой поток (`dialogue-import --raw`); меню из четырёх своих подписей показано в игре ([диалоги](../systems/dialogues.md)).
- **Пример** `examples/trader`: мод одной командой, ставится менеджером модов; на стенде — торговка, меню, покупки сообщениями ([опыт](../experiments/trader-handing-out.md)).
- **Адреса**: [выдача](../exe/items-and-money.md), [диалоги](../exe/dialogues.md).

## Открытые вопросы после этапа

- [Чем на стенде выбрать вариант в меню разговора](../questions/f2f-choice-input-on-bench.md)
- [Чем выдать навык `UpgradeData` в одиночной игре](../questions/upgrade-grant-single-player.md)
- [Как из логики проверить, хватает ли денег](../questions/money-check-in-logic.md)
- [Названия остальных «переменных» диалогов](../questions/dialogue-condition-variables.md)
- [Лечение аптечки на стенде](../questions/item-cash-and-health-pack.md)
- [Объекты игры в Lua и догадка о GUID](../questions/lua-groups-and-objects.md) — не проверялась

## Правила этапа

Этап обязан обновить базу знаний: вопросы получают новый статус, появляются новые заметки, у каждого утверждения есть источник. См. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
