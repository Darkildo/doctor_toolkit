---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58, 59]
sources: ["docs/analysis/renderer.md §4", "docs/analysis/renderer.md §9", "docs/analysis/performance.md §8", "docs/analysis/performance.md §10", "docs/analysis/performance.md §10.2", "docs/analysis/live-link.md §16.1", "docs/analysis/live-link.md §16.5", "docs/analysis/game-bench.md §29", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/render.rs"]
tags: [exe, render, vsync, fps]
updated: 2026-11-16
---
# Вертикальная синхронизация и ограничитель частоты

Адреса — виртуальные, издание PC 1.0.789.0. Настройка «синхронизация» — это замок 30 к/с: во весь экран `D3DPRESENT_INTERVAL_TWO`, в окне игра сама ждёт 33,3 мс занятым ожиданием. Модуль рендера снимает замок патчами интервала и ограничителя.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00F15A39` | данные: байт «синхронизация» из настроек (по умолчанию 1) | renderer.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0060E680` | запись байта синхронизации | renderer.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0060E690` | чтение байта синхронизации | renderer.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x0120587F` | данные: байт «весь экран» | renderer.md §4 |
| `0x0060E850` | запись байта «весь экран» | renderer.md §4 |
| `0x0060E860` | чтение байта «весь экран» | renderer.md §4 |
| `0x00F15A48` | данные: `синхронизация ? 2 : 0` — сколько «обновлений по 16,667 мс» ждать | renderer.md §4; crates/doctor-toolkit-live/src/render.rs |
| `0x00607B5A`, `0x006084D6` | места записи `0x00F15A48` (по renderer.md) | renderer.md §4 |
| `0x00607B59`, `0x006084CE` | непосредственные операнды `2` команд `and eax, 2`, дающих `0x00F15A48`; патч модуля рендера: `1` | performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x00607B40` | начало места создания устройства с настройкой синхронизации (сверяется при проверке издания модуля рендера) | crates/doctor-toolkit-live/src/render.rs |
| `0x006084C8` | начало такого же места в функции настроек `0x006082C0` | crates/doctor-toolkit-live/src/render.rs |
| `0x0120575A` | данные: байт «ждёт сама игра» (окно или устройство без `INTERVAL_TWO`) | renderer.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x00608CA0` | ограничитель: пока прошло меньше `n × 16,6667` мс от прошлого показа — `SleepEx(0)` и снова `QueryPerformanceCounter` | renderer.md §4; crates/doctor-toolkit-live/src/addr.rs; crates/doctor-toolkit-live/src/render.rs |
| `0x00608CAC` | место ограничителя с проверкой байта `0x0120575A` | crates/doctor-toolkit-live/src/addr.rs |
| `0x00608DA2` | конец ограничителя: QPC этого показа → `0x01223530` | crates/doctor-toolkit-live/src/render.rs |
| `0x01223530` | данные: QPC последнего прохода ограничителя (`u64`) | crates/doctor-toolkit-live/src/render.rs |
| `0x00E4465C` | данные: константа 16,6667 мс | renderer.md §4; crates/doctor-toolkit-live/src/render.rs |
| `0x006075E2` | непосредственный операнд `2` команды `mov [esi+0x34], 2` (`INTERVAL_TWO`); патч модуля рендера: `1` (`INTERVAL_ONE`) | renderer.md §9; performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x006075DF` | команда, ставящая `D3DPRESENT_INTERVAL_TWO` в параметрах показа | crates/doctor-toolkit-live/src/render.rs |
| `0x006075D7` | начало этого места (сверяется при проверке издания модуля рендера) | crates/doctor-toolkit-live/src/render.rs |

## Подробнее

- [Рендер](../../analysis/renderer.md) §4, §9
- [Производительность](../../analysis/performance.md) §8, §10, §10.2
