---
type: question
status: hypothesis
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 58, 101]
sources: ["docs/analysis/renderer.md §3"]
tags: [question, render]
updated: 2026-11-16
---
# Порядок команд внутри проходов кадра

Сообщения кадра (`iMsgBeforeRender`, `iMsgPreRender`, `iMsgDoRender`, `iMsgDoRenderFullScreen`, `iMsgDoRenderFullScreenDebug`, `iMsgPreShowRaster`) по именам — проход сцены, полноэкранный проход и шаг перед показом (гипотеза); порядок команд внутри них не разобран.

## Следующий шаг

Снять последовательность вызовов D3D9 кадра графическим модулем и разложить по проходам — нужно для сокращения числа вызовов отрисовки.

Запланировано: [Этап 101: Меньше вызовов отрисовки](../research/stage-101.md).

## Связанное

- [Рендер](../engine/render.md)
- [Сообщения кадра: `iMsgBeforeRender` и другие](../messages/frame-messages.md)
- [Поток рендера и буферы команд кадра](../exe/render-thread.md)
- [docs/analysis/renderer.md](../../analysis/renderer.md) §3, §8
