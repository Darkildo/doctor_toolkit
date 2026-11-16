---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 32]
sources: ["docs/analysis/launch-flags.md §4"]
tags: [exe, debug, leftovers]
updated: 2026-11-16
---
# Отладочные средства выпущенной сборки

Адреса — виртуальные, издание PC 1.0.789.0. Отладочное меню, подстройка на ходу и визуальный отладчик Havok в финальной сборке не работают: остались данные и неиспользуемый код.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00E514E8` | данные: начальное значение буфера `personaldebugmenu.txt` (22 байта); на буфер никто не ссылается | launch-flags.md §4 |
| `0x00E51480`, `0x00E51600` | данные: границы диапазона, перебранного в поиске ссылок на буфер меню | launch-flags.md §4 |
| `0x005A0707` | единственное совпадение перебора — байты команды `call`, не ссылка | launch-flags.md §4 |
| `0x00DD33D4` | данные: таблица методов `hkBsdSocket` | launch-flags.md §4 |
| `0x00C72A30` | `listen` сокета Havok («Listening on host[…] port») | launch-flags.md §4 |
| `0x00C721E0` | фабрика `hkSocket::create`; не вызывается ни разу | launch-flags.md §4 |
| `0x00EFB8FC` | данные: указатель на фабрику `hkSocket::create`; никто не читает | launch-flags.md §4 |
| `0x00C807A0` | второй `bind`/`listen` — DirtySock, сеть мультиплеера | launch-flags.md §4 |

## Подробнее

- [Флаги запуска](../../analysis/launch-flags.md) §4 «Отладочные средства»
