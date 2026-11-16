---
type: message
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58]
sources: ["docs/analysis/renderer.md §3", "docs/analysis/live-link.md §12.2"]
tags: [messages, engine, render]
updated: 2026-11-16
---
# Сообщения кадра: `iMsgBeforeRender` и другие

Сообщения кадра, которые движок шлёт на главном потоке (хэши регистрирует `0x0040D640`): `iMsgBeforeRender`, `iMsgPreRender`, `iMsgDoRender`, `iMsgDoRenderFullScreen`, `iMsgDoRenderFullScreenDebug`, `iMsgPreShowRaster`.

По именам — проход сцены, полноэкранный проход (постэффекты, интерфейс) и шаг перед показом; это гипотеза, порядок команд внутри проходов не разобран. Модуль связи использует `iMsgBeforeRender` (переменная с хэшем `0x01206960`) для свободной камеры.

## Связанное

- [Рендер](../engine/render.md)
- [Главный цикл](../engine/main-loop.md)
- [Поток рендера и буферы команд кадра](../exe/render-thread.md)
- [Шина сообщений: имена, подписка, отправка](../exe/message-bus.md)
- [Порядок команд внутри проходов кадра](../questions/render-pass-command-order.md)
- [Сообщения движка: полный список](engine-messages.md)
- [docs/analysis/renderer.md](../../analysis/renderer.md) §3
