---
type: message
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 45]
sources: ["docs/scripts.md §1", "docs/analysis/queens.md §1", "docs/mcp.md §9"]
tags: [messages, fade, triggers]
updated: 2026-11-16
---
# Сообщение `boundary_fade_out`

Сообщение, по которому экран гаснет (`FadeScreenFx`). Его посылают объёмы `TriggerHurt` на границе мира: в `gf2/queens/local` таких 11, они лежат под водой и доходят до высоты ≈0,9 м — игрок в воде гаснет и возвращается.

Проверено на стенде (этап 45): граф скрипта «On message `doctor_trig_enter` → Send message `boundary_fade_out`» гасит экран (средняя яркость кадра 106,5 → 1,6). Через MCP `send_message boundary_fade_out` находит 1 слушателя, следующий снимок почти чёрный (яркость 1,4). Удобная проверка того, что сообщение дошло и действие видно в кадре.

## Связанное

- [Шина сообщений](../engine/message-bus.md)
- [Lua в игре](../engine/lua.md)
- [Свои скрипты Lua: где они запускаются и что нужно для этого](../experiments/lua-own-scripts.md)
- [`SetTimeOfDay` и `UIFanfare` из скрипта в кадре не сверялись](../questions/lua-settimeofday-uifanfare-in-frame.md)
- [docs/scripts.md](../../scripts.md) §1
