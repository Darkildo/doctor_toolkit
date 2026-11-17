---
type: research
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [101]
sources: ["docs/analysis/renderer.md §11", "docs/analysis/performance.md §11", "docs/analysis/game-bench.md §38", "docs/analysis/game-bench.md §46", "research/d3d9_frame_trace.py", "research/draw_ab.py", "research/draw_threads.py"]
tags: [research, render, draw-calls]
updated: 2026-11-17
---
# Этап 101: Меньше вызовов отрисовки

- Состояние: сделан частично (2026-11-17): разбор и объединение сделаны, замер под DXVK снят — выигрыша во времени нет; инстансинг не делался.
- Зависит от: [этап 72](stage-72.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Запись всех вызовов D3D9 одного кадра графическим модулем и её разбор: проходы кадра по порядку, доли вызовов, функции игры, которые рисуют, — [рендер](../../analysis/renderer.md) §11, [вызовы отрисовки](../exe/draw-calls.md); порядок проходов — [вопрос, частично закрыт](../questions/render-pass-command-order.md).
- Объединение серий вызовов без смены состояния в один (`draw_merge` в `doctor_render.cfg`, `play --draw-merge`; по умолчанию выключено): на 15–26 % меньше вызовов до Direct3D, картинка в трёх городах та же, под DXVK и под wined3d — [опыт](../experiments/draw-call-merging.md).
- Замер под DXVK сериями «включено — выключено» в трёх городах: время кадра не меняется, ЦП потока рендера с объединением на 5–8 % больше, ЦП `dxvk-cs` на 0–1,3 % меньше — выигрыша нет; итоговая сборка устойчива (по 6,9 мин с переносами игрока в каждом городе) — [производительность](../../analysis/performance.md) §11.3–§11.4, [стенд](../../analysis/game-bench.md) §46.

## Что осталось

- [Окупается ли объединение на Direct3D 9 драйвера](../questions/draw-merge-on-native-d3d9.md) (Windows) и сколько стоят сами перехваты: не измерено; [частота на свободной машине и на Windows](../questions/frame-rate-on-free-machine.md) по-прежнему не измерена.
- [Инстансинг одинакового реквизита](../questions/draw-instancing.md) не сделан.
- [Что игра проверяет запросами видимости](../questions/occlusion-queries.md); [отказ выделения в DXVK](../questions/dxvk-buffer-allocation-failure.md).
- [Кадр прибытия зоны](../questions/main-thread-zone-arrival.md) этапом не разбирался.

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
