---
type: exe
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 58]
sources: ["docs/analysis/performance.md §3", "docs/analysis/performance.md §6", "docs/analysis/performance.md §9", "docs/analysis/live-link.md §16.6"]
tags: [exe, profile, performance]
updated: 2026-11-16
---
# Профиль: где стоят потоки

Адреса — виртуальные, издание PC 1.0.789.0. Выборка профиля показывает, где чаще всего находятся главный поток и поток рендера. Точки перехвата и счётчики профиля описаны в заметках о показе кадра, чтении файлов и часах игры.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0045E670` | вызов обновления объекта (9–39 % выборок главного потока) | performance.md §3 |
| `0x00808540` | функция, из которой зовётся `0x0045E670` | performance.md §3; performance.md §9 |
| `0x00809323` | место вызова `0x0045E670` в `0x00808540` | performance.md §3 |
| `0x0083CBB0` | функция, связанная с сообщением `iMsgTrafficMgrDespawnEntities` (плотность толпы и машин; гипотеза источника) | performance.md §6 |

## Что неизвестно

Что ждёт главный поток под `0x00808540` (до 39 % выборок) — не выяснено. Параметры плотности толпы не найдены.

## Подробнее

- [Производительность](../../analysis/performance.md) §3, §6, §9
- [Живая связь](../../analysis/live-link.md) §16
