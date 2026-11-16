---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 35, 58, 59]
sources: ["docs/analysis/live-link.md §3", "docs/analysis/live-link.md §12.2", "docs/analysis/renderer.md §5", "docs/analysis/performance.md §10.3", "docs/analysis/game-bench.md §29", "docs/analysis/game-bench.md §31.2", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/render.rs", "crates/doctor-toolkit-render-asi/src/fov.rs"]
tags: [exe, camera, fov, view]
updated: 2026-11-16
---
# Камера: менеджер камер, виды, угол обзора

Адреса — виртуальные, издание PC 1.0.789.0. Камера, которой рисует игра, — слот пула видов; вид задаётся штатным сеттером матрицы, у потока рендера свои копии видов. Угол обзора картинки в игре меняет единственный вызов `SetFov` — в менеджере камер.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0122339C` | данные: менеджер камер (указатель); при `+0x154 ≠ 0` текущая камера в `+0x14`, иначе слот 0 TLS потока `+0x24` | live-link.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x01207DB0` | данные: вид камеры в уровне — 12-й слот пула видов: `+0` обратная матрица, `+0x40` мировая, `+0x80` шесть плоскостей | live-link.md §3; live-link.md §12.2 |
| `0x01206BB0` | данные: пул видов (16 видов по 0x180 байт) | live-link.md §12.2 |
| `0x012053B4` | данные: флаги занятости пула видов | live-link.md §12.2 |
| `0x004224F0` | выделение вида из пула | live-link.md §12.2 |
| `0x00422590` | новый вид (слот пула): `+0x148` дальняя плоскость 5000, `+0x14C` угол 1,0472 (60°), `+0x150` соотношение | live-link.md §12.2; renderer.md §5 |
| `0x01223398` | данные: объект с картой копий видов потока рендера (`+8`) | live-link.md §12.2 |
| `0x0110B690` | данные: команда «мировая матрица вида» для копии вида потока рендера | live-link.md §12.2 |
| `0x0110B67C` | данные: команда «обратная матрица вида» | live-link.md §12.2 |
| `0x00422C50` | запись команды с матрицей вида для потока рендера | live-link.md §12.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00422D40` | сеттер мировой матрицы вида: `EAX` — вид, `ESI` — матрица (выровнена на 16), в стеке `bool invert`, `RET 4` | live-link.md §12.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00422DA9` | место сеттера, где пишется команда потоку рендера и `RET 4` | crates/doctor-toolkit-live/src/edition.rs |
| `0x0056C640` | обратная матрица (из сеттера вида) | live-link.md §12.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00423900` | пересчёт плоскостей и проекции вида | live-link.md §12.2; renderer.md §5 |
| `0x00424D70` | менеджер камер на `iMsgRunningPostTick` (→ `0x004252E0`) | live-link.md §12.2 |
| `0x004252E0` | менеджер камер: список камер по 0x1D0 байт (угол `+0x30`, режим `+0x38`) | live-link.md §12.2; renderer.md §5 |
| `0x00425EA0` | менеджер камер пишет вид, которым рисуется кадр | live-link.md §12.2; renderer.md §5 |
| `0x00425FCF` | начало места вызова `SetFov` в `0x00425EA0` (сверяется при проверке издания модуля рендера) | crates/doctor-toolkit-live/src/render.rs |
| `0x00425FE4` | `call 0x00422FB0` в менеджере камер — единственный вызов `SetFov`, меняющий картинку в игре; перехват модуля рендера | renderer.md §5; performance.md §10.3; crates/doctor-toolkit-live/src/render.rs |
| `0x00422FB0` | `SetFov(угол, соотношение)` вида, `thiscall`: `+0x14C`, `+0x150`, пересчёт проекции | renderer.md §5; crates/doctor-toolkit-live/src/render.rs |
| `0x00425240` | пересчёт угла с опорой 16:9 или 4:3 (для режима 2) | renderer.md §5 |
| `0x00E447AC` | данные: константа 1,7778 (16:9) | renderer.md §5 |
| `0x00E447A8` | данные: константа 1,3333 (4:3) | renderer.md §5 |
| `0x01205878` | данные: соотношение сторон `w / h` (`float`) | renderer.md §2; renderer.md §5 |
| `0x009BD990`, `0x009BEB00` | методы камеры игрока, зовущие `SetFov`; пешком в игре не вызываются | renderer.md §5; performance.md §10.3 |
| `0x009BDC40`, `0x009BE4B0` | ещё два метода камеры игрока, не вызывавшиеся при трассировке | performance.md §10.3 |
| `0x009BDA38`, `0x009BDE5D`, `0x009BE677`, `0x009BEB79` | места вызова `SetFov` в трассировке модуля (вызовы в диапазоне методов камеры игрока) | crates/doctor-toolkit-render-asi/src/fov.rs |
| `0x00413517`, `0x00413698`, `0x0041374B` | три места вызова `SetFov` `0x0041xxxx`: вызываются один раз (кубические карты) | crates/doctor-toolkit-render-asi/src/fov.rs; performance.md §10.3 |
| `0x00940DD8`, `0x0094639E` | места вызова `SetFov` каждый кадр, 80,2° (другой вид, на картинку не влияет) | performance.md §10.3 |
| `0x0044CD4F` | место вызова `SetFov` во время ролика (11 раз, 42°) | performance.md §10.3; game-bench.md §31.2 |
| `0x01205248` | данные: байт, который проверяет `0x0040E0A0` (слушатель звука по камере) | live-link.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x0060EA60` | берёт режимы адаптера для списка разрешений | renderer.md §5 |

## Что неизвестно

Не проверено, идут ли ролики через `0x00425240` с опорой 4:3. Признака «идёт ролик» для модуля не найдено.

## Подробнее

- [Живая связь](../../analysis/live-link.md) §3, §12.2 «Свободная камера»
- [Рендер](../../analysis/renderer.md) §5
- [Производительность](../../analysis/performance.md) §10.3
