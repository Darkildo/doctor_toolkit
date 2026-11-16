---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 46]
sources: ["docs/analysis/game-bench.md §24", "docs/analysis/live-link.md §14.6"]
tags: [mods, redirect, live-link, createfile, asi-loader]
updated: 2026-11-16
---
# Моды без записи в игру: подмена файлов модулем (этап 46)

Быстрый стенд; мод формата 2 «Havana pink»: 15 текстур потока `gf2/cuba/local/yuri/high/terrain3`
перекрашены в пурпурный, новый поток `gf2/doctor/s46` с клоном текстуры; `mods install` в менеджер
модов тени (`mods apply` не делался ни разу). Состояние тени до и после — SHA-256 каждого файла и цели
ссылок (3 751 запись) плюс `verify`.

| Прогон | Результат |
|---|---|
| первая версия (подмена слотов импорта из `DllMain`) | ни одного открытия не увидено: `live peek 0x00D55120 4` — `0x7BB9B040` (`kernel32.dll`), загрузчик ASI вернул слоты. Переделано на места вызова |
| журнал открытий (`--files-log`) | 77 открытий, все `CreateFileW from 0x00ac0bdf` (открытие файловой системы игры `0x00AC0AD0`); `CreateFileA` и `GetFileAttributesA` — ни разу |
| контроль: только `cmd.txt` подменой | игра сразу в Гаване, `cmd.txt` в папке игры нет; агава у ворот зелёная |
| мод, GF2ASI выключен | 5 подмен (`terrain3.str`, `cuba.toc`, `cuba_global.toc`, `doctor/s46.str`, `cmd.txt`); агава у ворот пурпурная; `live stream load gf2/doctor/s46` — загружен за 23 кадра (состояние 3); 770 с без исключений; после `uninstall` изменился только `godfather2.dxvk-cache` |
| мод, GF2ASI включён | то же; 399 с без исключений; изменился только `GF2_Hook.txt` |
| `play --mods --stream gf2/cuba` | `staged: 5 file(s) … (2 stream(s) rebuilt)`; после выхода `removed the staged files; the game opened 3 of the 5 redirected file(s), its folder was not written`; состояние тени совпало побайтно |

Что из этого следует:

- Ultimate ASI Loader возвращает слоты `CreateFileA`/`CreateFileW` исполняемого файла после загрузки
  модулей — перехват в слот из `DllMain` теряется.
- Игра открывает все свои данные одним местом и только на чтение: подмены `ReadFile` не нужны.
- Поток, которого нет в папке игры, игра загружает из подмены.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §24
- [Живая связь](../../analysis/live-link.md) §14
- [Моды](../../mods.md)
