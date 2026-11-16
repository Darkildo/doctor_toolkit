---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 49]
sources: ["docs/analysis/live-link.md §11.1", "docs/analysis/live-link.md §11.6", "docs/formats/mission-logic.md §3", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, journal, debug, print]
updated: 2026-11-16
---
# Журнал игры: отладочная печать и Lua print

Адреса — виртуальные, издание PC 1.0.789.0. Отладочный вывод выпущенной сборки почти весь вырезан; живых каналов два — `OutputDebugStringA` через слот импорта и Lua `print` игры через указатель обратного вызова. Модуль подменяет оба и зовёт прежние значения.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00D550C4` | данные: слот импорта `OutputDebugStringA` (KERNEL32); модуль кладёт в него свою функцию | live-link.md §11.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00E4E65C` | данные: ячейка таблицы поиска импорта для слота `0x00D550C4` | live-link.md §11.1; crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4EDB8` | данные: имя импорта `OutputDebugStringA` | live-link.md §11.1; crates/doctor-toolkit-live/src/edition.rs |
| `0x0065BC40` | printf отладки (свой буфер 4 КБ), печатает через слот `OutputDebugStringA` | live-link.md §11.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0065BCD5` | место в `0x0065BC40`, где зовётся `OutputDebugStringA` через слот | crates/doctor-toolkit-live/src/edition.rs |
| `0x01205A54` | данные: слушатели printf отладки | live-link.md §11.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0064D030` | статистика памяти (печатает через `0x0065BC40`) | live-link.md §11.1 |
| `0x0065E570` | таймеры (печатают через `0x0065BC40`) | live-link.md §11.1 |
| `0x00655EF0` | сеть (печатает через `0x0065BC40`) | live-link.md §11.1 |
| `0x00AF0C90` | печать FESL через тот же слот | live-link.md §11.1 |
| `0x006458F0`, `0x00649DB0` | проверки с `int3` (assert), печатают через тот же слот | live-link.md §11.1 |
| `0x008D1330` | `Debug::Printf`, скомпилированная в пустой `ret` (строки статистики «---Player Rank,%d») | live-link.md §11.1 |
| `0x005CCF50` | пустая функция печати («APTCallback_AssertFail:%s», «%s Shader Supports …») | live-link.md §11.1 |
| `0x0046BCE0` | Lua `print` игры: склеивает аргументы, берёт скрипт и строку вызвавшего и зовёт указатель `0x0110A6C0` | live-link.md §11.1; mission-logic.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x0062D560` | стандартная `print` Lua (пишет в `stdout`), игрой не используется | live-link.md §11.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0110A6C0` | данные: указатель обратного вызова печати `cdecl (скрипт, строка, what, текст)`; модуль кладёт в него свою функцию | live-link.md §11.1; mission-logic.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x0046BF56` | начало места в `print`, где зовётся указатель `0x0110A6C0` | live-link.md §11.1; crates/doctor-toolkit-live/src/edition.rs |
| `0x0046BF79` | конец этого места (`0x0046BF56`–`0x0046BF79`) | live-link.md §11.1 |
| `0x0046BF77` | место вызова обратного вызова печати (по коду модуля) | crates/doctor-toolkit-live-asi/src/journal.rs |
| `0x0046BFC0` | пустая функция (`ret`), которая стоит в указателе в выпущенной сборке | live-link.md §11.1; mission-logic.md §3; crates/doctor-toolkit-live/src/addr.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §11.1 «Что игра печатает»
- [Логика миссий](../../formats/mission-logic.md) §3.1
