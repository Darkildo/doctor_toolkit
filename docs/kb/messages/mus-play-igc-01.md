---
type: message
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 27, 29]
sources: ["docs/formats/mission-logic.md §4.1", "docs/logic-graph.md §8", "docs/formats/cutscenes.md §2.1, §6.1"]
tags: [messages, music, cutscenes]
updated: 2026-11-16
---
# Сообщение `mus_play_igc_01`

Сообщение запуска музыки ролика `igc01`. В `cuba/local` его слушает `MultiManager` и рассылает `mus_stop_car_radio_content`, `aud_start_mission_1_script` (через 0,2 с) и `mus_start_underscore` (через 0,3 с).

Пример сообщения, которое «слышат, но никто не посылает»: среди объектов SimGroup отправителя нет — его шлёт ролик (`trinity_seq_master`) текстовой командой `0x402` вида 1 (сообщение на общую шину). У `igc01` отдельной музыки нет: сообщение только включает флаг миссии.

## Связанное

- [Шина сообщений](../engine/message-bus.md)
- [Класс `MultiManager`](../classes/MultiManager.md)
- [Ролики](../systems/cutscenes.md)
- [Сообщения, которые посылают не SimGroup: Lua, ролики, `score`](../questions/non-simgroup-message-senders.md)
- [docs/formats/mission-logic.md](../../formats/mission-logic.md) §4.1
- [docs/formats/cutscenes.md](../../formats/cutscenes.md) §2.1, §6.1
- [docs/logic-graph.md](../../logic-graph.md) §8
