---
type: question
status: unknown
confidence: exe
editions: ["1.0.789.0"]
stages: [75]
sources: ["docs/analysis/handing-out.md §4", "research/items_survey.py"]
tags: [question, economy, simgroup]
updated: 2026-11-16
---
# Как из логики проверить, хватает ли игроку денег

`MoneyAdaptor` с отрицательной суммой списывает не больше, чем есть (`0x008A6EE0` ограничивает сумму книгами семьи), но отказа не даёт: сообщения «денег не хватило» нет. Класса-условия по деньгам в SimGroup не найдено (`TestFamilySettings` в файлах игры проверяет состояние семей — убит ли босс, открыта ли; `TestScore` не читан), Lua денег не видит. Торговец из примера поэтому отдаёт товар и при пустом кошельке.

## Следующий шаг

Прочитать читатели и обработчики `TestScore`, `ThresholdTrackingEventTransmitter` и `StatAdaptor` (статистика `MONEY_CURRENT_FUNDS` — функция `0x009075A0` таблицы `0x00E547F8`): если порог можно повесить на статистику денег, условие собирается из них. Иначе — дать Lua функцию денег модулем связи.

## Связанное

- [Экономика: деньги, заведения, награды](../systems/economy.md)
- [Выдача предметов, денег и улучшений: адреса](../exe/items-and-money.md)
- [Класс `MoneyAdaptor`](../classes/MoneyAdaptor.md)
- [Этап 75](../research/stage-75.md)
