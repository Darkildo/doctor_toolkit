---
type: question
status: unknown
confidence: exe
editions: ["1.0.789.0"]
stages: [75]
sources: ["docs/analysis/handing-out.md §5", "research/exe_dis.py"]
tags: [question, upgrades, simgroup]
updated: 2026-11-16
---
# Чем выдать навык `UpgradeData` в одиночной игре

Улучшения семьи — список номеров (`семья+0x124`, число `+0x128`); его проверяет `TestUpgrade`. Выдаёт улучшение игроку `0x007A6390(игрок, номер)`, а зовут её только обработчик сетевого запроса и `DonControlGadget` (`0x0089B970`) — устройство «взгляда дона» сетевых карт (90 объектов, все в `mp_*`). Объекта логики одиночной игры, который дал бы навык, не найдено; у Lua такой функции нет.

Уровень оружия — другое «улучшение»: оружие уровнем выше выдаётся `GiveItemRelay` с флагом 4 и заменяет прежнее ([оружие](../systems/weapons.md)) — проверено на стенде.

Там же открыто: что шлёт `Item.15`; что делают `HonorAdaptor`, `ScoreAdaptor`, `StatAdaptor` вне сетевой игры.

## Следующий шаг

Скопировать `DonControlGadget` сетевой карты в уровень одиночной игры (режим `DonControlGadget.0` = 1, улучшение — атрибут 5) и проверить `TestUpgrade` после срабатывания; если не работает — вызывать `0x007A6390` из модуля связи (новая команда `live upgrade`).

## Связанное

- [Навыки и улучшения](../systems/upgrades.md)
- [Выдача предметов, денег и улучшений: адреса](../exe/items-and-money.md)
- [Класс `DonControlGadget`](../classes/DonControlGadget.md)
- [Класс `TestUpgrade`](../classes/TestUpgrade.md)
- [Этап 75](../research/stage-75.md)
