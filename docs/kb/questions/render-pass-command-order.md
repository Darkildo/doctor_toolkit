---
type: question
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 58, 101]
sources: ["docs/analysis/renderer.md §3", "docs/analysis/renderer.md §11.2", "research/d3d9_frame_trace.py"]
tags: [question, render]
updated: 2026-11-16
---
# Порядок команд внутри проходов кадра

Порядок проходов кадра установлен записью вызовов D3D9 графическим модулем в трёх городах (этап 101): три цели 256×256 (уменьшенная сцена) → глубина и нормали → карта теней (три каскада 832×832 в одной цели 832×2496) → буфер света → освещённый проход, прозрачное, частицы и интерфейс в цель сцены → постэффекты. Доли вызовов по проходам и функции игры, которые их делают, — [рендер](../../analysis/renderer.md) §11.2, §11.3 и [вызовы отрисовки](../exe/draw-calls.md).

Не установлено: какое сообщение кадра (`iMsgBeforeRender`, `iMsgPreRender`, `iMsgDoRender`, `iMsgDoRenderFullScreen`, `iMsgDoRenderFullScreenDebug`, `iMsgPreShowRaster`) отвечает какому проходу и где в освещённом проходе кончается сцена и начинается интерфейс (место для постэффектов без HUD).

## Следующий шаг

В записи кадра для вызовов освещённого прохода взять более длинные цепочки адресов возврата и сопоставить их с обработчиками сообщений кадра; первую команду интерфейса искать по шейдерам APT.

## Связанное

- [Рендер](../engine/render.md)
- [Сообщения кадра: `iMsgBeforeRender` и другие](../messages/frame-messages.md)
- [Поток рендера и буферы команд кадра](../exe/render-thread.md)
- [Вызовы отрисовки](../exe/draw-calls.md)
- [docs/analysis/renderer.md](../../analysis/renderer.md) §3, §8, §11
