---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58, 35]
sources: ["docs/analysis/renderer.md §1", "docs/analysis/renderer.md §3", "docs/analysis/live-link.md §2", "docs/analysis/live-link.md §12.2", "docs/analysis/live-link.md §12.3", "crates/doctor-toolkit-live/src/addr.rs"]
tags: [exe, render, threads, commands]
updated: 2026-11-16
---
# Поток рендера и буферы команд кадра

Адреса — виртуальные, издание PC 1.0.789.0. Главный поток пишет команды кадра в буфер, поток рендера (`Presentation`) их исполняет — весь D3D идёт там. Буфер команд кадра задан только внутри кадра, поэтому запись в игру модуль делает в перехвате обновления `GameMainLoop`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00485AE0` | тело потока рендера: `TLS+0x34 = 1`, цикл исполнения буферов команд | renderer.md §3; live-link.md §12.3 |
| `0x01205420` | данные: номер ядра для маски сродства потока рендера | renderer.md §3 |
| `0x00E2E294` | данные: семафор потока рендера | renderer.md §3 |
| `0x00E2E298` | данные: семафор, которого ждёт начало кадра | renderer.md §3 |
| `0x01206870` | данные: очередь готовых буферов команд | renderer.md §3 |
| `0x01206878` | данные: список свободных буферов команд | renderer.md §3 |
| `0x00485280` | отложенные вызовы потока рендера (после буфера кадра) | renderer.md §2; renderer.md §3 |
| `0x01206880` | данные: буфер команд кадра (указатель); задан только внутри кадра | live-link.md §2; renderer.md §1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0068BAF0` | начало кадра на главном потоке: счётчик кадров += 1, взять свободный буфер, первые команды | renderer.md §3; live-link.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0040CB40`, `0x0040CBE0` | взять свободный буфер команд | renderer.md §3 |
| `0x0068BCD0` | конец кадра: последняя команда, буфер в очередь потока рендера, ожидание | renderer.md §3; live-link.md §2 |
| `0x0040CB90` | буфер кадра — в очередь потока рендера | renderer.md §3; live-link.md §12.3 |
| `0x00485D50` | отдать буфер и ждать семафор, если включён поток рендера | renderer.md §3 |
| `0x012053CB` | данные: байт «поток рендера включён» | renderer.md §3 |
| `0x00AB5130` | семафор: ждать (таймаут по `GetTickCount`) | renderer.md §3 |
| `0x00AB51E0` | семафор: отпустить | renderer.md §3 |
| `0x00AB4DB0` | замок: `TryEnterCriticalSection`, при таймауте — `Sleep(1)` в цикле | renderer.md §3 |
| `0x0040E1F0` | отрисовка кадра уровня: первым делом шлёт `iMsgBeforeRender`, затем выбирает вид и пишет команды кадра | live-link.md §12.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0040E222` | место `call 0x00408A00` с `iMsgBeforeRender` в отрисовке кадра | crates/doctor-toolkit-live/src/addr.rs |

## Что неизвестно

Порядок команд внутри сообщений кадра (`iMsgPreRender`, `iMsgDoRender`…) не разобран.

## Подробнее

- [Рендер](../../analysis/renderer.md) §3 «Поток рендера и буферы команд»
- [Живая связь](../../analysis/live-link.md) §2, §12.2
