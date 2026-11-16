---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 33]
sources: ["docs/analysis/game-bench.md §19"]
tags: [dxvk, vulkan, xvfb, frame-rate]
updated: 2026-11-16
---
# DXVK и 32-битный Vulkan: частота кадров на стенде

Доработка этапа 33 (2026-11-13). На машину поставлены 32-битные библиотеки драйвера NVIDIA
(`libnvidia-gl-595:i386`) и `xvfb`; DXVK v2.6.2 — из Lutris, в систему и префиксы не ставился.
Стенд получил `DXVK=1` и `SERVER=xvfb`.

Игра (`-stream gf2 cuba`, Гавана у ворот аэропорта):

| Сервер | Размер | Кадров/с | Время кадра |
|---|---|---|---|
| Xephyr | 800×600 | 61 | 14,6–20,4 мс |
| Xephyr | 1280×720 | 26,5–31,9 (49 при взгляде в сторону) | 19–54 мс |
| Xvfb | 1280×720 | 27,2–35,1 | 27–43 мс |
| до DXVK (llvmpipe) | 1280×720 | 1–2 | — |

- В Xvfb `vulkaninfo` видит RTX 4070; тестовая программа d3d9 с DXVK: Xvfb — 53 к/с, Xephyr — 49 к/с.
- Частота падает почти пропорционально числу точек; нижняя граница времени кадра ~28–31 мс — предел
  задаёт вывод кадра в сервер без DRI3 (копия через процессор), не видеокарта.
- 320 с в уровне без падений; после прогона `d3d9.dll` из тени убран.
- CLI: `play --method wine --prefix <стенд> --dxvk --stream gf2/cuba --resolution 1280x720` — во время
  игры `d3d9.dll` в папке, `WINEDLLOVERRIDES='dinput8=n,b;d3d9=n,b'`; после выхода «took away
  d3d9.dll».

Не сработало до этого: Vulkan-рендерер wined3d (`WINE_D3D_CONFIG=renderer=vulkan`) падает на шейдерах
d3d9; RADV встроенной графики недоступен.

Что не проверено: вложенный сервер с DRI3 (Xwayland в безголовом компоновщике, gamescope
`--headless`), который дал бы вывод без копии.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §17, §19
- [Производительность](../../analysis/performance.md) §2
