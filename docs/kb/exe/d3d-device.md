---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58, 59]
sources: ["docs/analysis/renderer.md §2", "docs/analysis/renderer.md §8", "docs/analysis/performance.md §10.4", "docs/analysis/live-link.md §12.3", "crates/doctor-toolkit-live/src/render.rs"]
tags: [exe, render, d3d9, device]
updated: 2026-11-16
---
# Устройство Direct3D 9: создание, сброс, цели кадра

Адреса — виртуальные, издание PC 1.0.789.0. Игра создаёт `IDirect3DDevice9` с флагами `HARDWARE_VERTEXPROCESSING | PUREDEVICE | MULTITHREADED`. Сцена рисуется не в задний буфер, а в свою цель A8R8G8B8 размером с экран.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00607990` | создание устройства: `Direct3DCreate9`, `CreateDevice`, проверка формата R32F, `CreateQuery` | renderer.md §2; live-link.md §12.3 |
| `0x00607999` | место чтения размера заднего буфера | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x012058CC` | данные: `IDirect3D9` | renderer.md §2 |
| `0x01205750` | данные: `IDirect3DDevice9*` | renderer.md §2; live-link.md §12.3; crates/doctor-toolkit-live/src/render.rs |
| `0x0120575C` | данные: номер адаптера | renderer.md §2 |
| `0x012058B4` | данные: окно устройства (`HWND`) | renderer.md §2; crates/doctor-toolkit-live/src/render.rs |
| `0x011D9188` | данные: `D3DPRESENT_PARAMETERS` устройства (`Windowed` `+0x20`, `PresentationInterval` `+0x34`) | renderer.md §2; crates/doctor-toolkit-live/src/render.rs |
| `0x011D91A8` | данные: поле `Windowed` параметров показа | performance.md §10.2; crates/doctor-toolkit-live/src/render.rs |
| `0x011D91BC` | данные: поле `PresentationInterval` параметров показа | crates/doctor-toolkit-live/src/render.rs |
| `0x0120575B` | данные: байт «выборка текстур в вершинном шейдере» | renderer.md §2 |
| `0x01205754` | данные: запрос `D3DQUERYTYPE_EVENT` | renderer.md §2 |
| `0x006073E0` | параметры показа: один задний буфер, без MSAA, `D3DSWAPEFFECT_DISCARD`; выбирает интервал показа | renderer.md §2; renderer.md §4 |
| `0x0060E810` | размер заднего буфера | renderer.md §2 |
| `0x012058B8` | данные: ширина заднего буфера (`u16`) | renderer.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x012058BC` | данные: высота заднего буфера (`u16`) | renderer.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0120587C`, `0x01205880` | данные: размер вида (равен заднему буферу); его читают около 40 мест | renderer.md §8; performance.md §10.4 |
| `0x00986D10` | применение настроек экрана: окно, синхронизация, ширина, высота, частота → сброс устройства и частота такта | renderer.md §2 |
| `0x00607790` | сброс устройства (из применения настроек) | renderer.md §2 |
| `0x00608250` | `Reset` устройства с теми же параметрами | renderer.md §2; crates/doctor-toolkit-live/src/render.rs |
| `0x00608257` | `call 0x00607E10` в `Reset`; перехват модуля рендера (сначала свои ресурсы `DEFAULT`) | performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x00607E10` | освобождает цели в пуле `DEFAULT` перед `Reset` | renderer.md §2; crates/doctor-toolkit-live/src/render.rs |
| `0x0060827A` | место `Reset`: байт `0x0120575A` после сброса (сверяется при проверке издания модуля рендера) | crates/doctor-toolkit-live/src/render.rs |
| `0x006082C0` | функция настроек, пишущая `0x00F15A48` | crates/doctor-toolkit-live/src/render.rs |
| `0x00608F10` | цели рендера кадра (создаются через `0x00606C80`) | renderer.md §2 |
| `0x012058A4` | данные: цель цвета сцены A8R8G8B8 w×h (индекс в таблице текстур) | renderer.md §2; crates/doctor-toolkit-live/src/render.rs |
| `0x012058A8` | данные: цель глубины D24S8 w×h | renderer.md §2 |
| `0x012058AC` | данные: цель D24S8 1024×1024 (тень) | renderer.md §2 |
| `0x0119DA5E` | данные: байт «устройство потеряно» (`D3DERR_DEVICELOST` из `Present`) | renderer.md §2 |
| `0x011F39D8`, `0x011F3DD8` | данные: кэш состояний устройства | performance.md §10.4 |
| `0x011F38F0` | данные: кэш целей рендера | performance.md §10.4 |
| `0x006080A0` | сбрасывает кэш состояний после `Reset` | performance.md §10.4 |
| `0x0060F380` | флаг `-codepath`: пишет путь шейдеров в `0x00F15A18` | launch-flags.md §3 |
| `0x00F15A18` | данные: путь шейдеров (вместо выбора по возможностям видеокарты) | launch-flags.md §3 |
| `0x0060F280` | выбор пути шейдеров по возможностям видеокарты | launch-flags.md §3 |

## Подробнее

- [Рендер](../../analysis/renderer.md) §2 «Устройство и показ кадра», §8
- [Производительность](../../analysis/performance.md) §10.4
