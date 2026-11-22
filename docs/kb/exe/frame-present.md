---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58, 59, 114]
sources: ["docs/analysis/renderer.md §2", "docs/analysis/live-link.md §16.1", "docs/analysis/live-link.md §16.5", "docs/analysis/live-link.md §16.6", "docs/analysis/performance.md §3", "docs/analysis/performance.md §10", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/render.rs", "docs/analysis/dev-menu-overlay.md §1.1"]
tags: [exe, render, present, hooks]
updated: 2026-11-20
---
# Показ кадра: цепочка Present

Адреса — виртуальные, издание PC 1.0.789.0. В конце кадра главный поток ставит отложенный вызов, а поток рендера после буфера кадра выполняет цепочку показа: ограничитель, копия сцены в задний буфер, `Present`. В этой цепочке стоят перехваты профиля и модуля рендера.

| Адрес | Что это | Источник |
|---|---|---|
| `0x01117BB0` | данные: список отложенных вызовов потока рендера | renderer.md §2 |
| `0x0040E066` | место, где главный поток ставит `0x0040DF60` в список отложенных вызовов | renderer.md §2 |
| `0x0040E068`, `0x0040E816` | места, где стоит адрес `0x0040DF60` | live-link.md §16.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0040DF60` | обратный вызов конца кадра на потоке рендера: счётчики профилировщика игры, затем показ | renderer.md §2; live-link.md §16.1 |
| `0x01223380` | данные: профилировщик игры (указатель): `+0x9C`/`+0xA0`, QPC в `+0xA8`/`+0xB0` | renderer.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0040DFA2` | `call 0x0060D8B0` (`E8 09 F9 1F 00`); перехват профиля модуля: время до и после показа | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0060D8B0` | показ кадра: `cdecl (char, int)`; внутри ограничитель и копия с `Present` | live-link.md §16.1; renderer.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0060D8DA` | место в показе: вызов ограничителя, затем копии и `Present` (сверяется при проверке издания модуля рендера) | crates/doctor-toolkit-live/src/render.rs |
| `0x0060D8F2` | `call 0x00608CA0` (ограничитель) в показе; перехват модуля рендера | performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x0060D8FA` | `call 0x00608E90` (копия и `Present`) в показе; перехват модуля рендера (свой предел частоты) | performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x00608E90` | копия кадра и `Present`; в начале — `call 0x00608DC0`, перехват модуля рендера (FXAA и резкость) | renderer.md §2; performance.md §10; crates/doctor-toolkit-live/src/render.rs |
| `0x00608DC0` | копия цели сцены `*0x012058A4` в задний буфер (`D3DXLoadSurfaceFromSurface`) — место для постэффектов | renderer.md §1; renderer.md §8; crates/doctor-toolkit-live/src/render.rs |
| `0x00608E9E` | `mov eax,[0x01205750]` (`A1 50 57 20 01`): после копии и FXAA, перед `Present`, только при заданном окне; переходов внутрь нет; место оверлея платформы (план) | dev-menu-overlay.md §1.1 |
| `0x00608EB3` | место вызова `IDirect3DDevice9::Present` | performance.md §3; live-link.md §16.6 |
| `0x0060960F` | место вызова `SetScissorRect` | performance.md §3; live-link.md §16.6 |
| `0x0060CB5D`, `0x0060C8B7` | места прочих вызовов D3D на потоке рендера | performance.md §3 |
| `0x00608FD5`, `0x009CA969` | другие места вызова показа `0x0060D8B0` (профилем не измеряются) | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x009CA960` | обратный вызов экранов вне кадра (указатель в `0x00E79AA4`): `0x0060D8B0(1, 0)` по `0x009CA969`; в Ghidra функцией не заведён | dev-menu-overlay.md §1.1 |

## Подробнее

- [Рендер](../../analysis/renderer.md) §2 «Показ», §8
- [Живая связь](../../analysis/live-link.md) §16.5
- [Производительность](../../analysis/performance.md) §10
- [Оверлей мода читов](../../analysis/dev-menu-overlay.md) §1
