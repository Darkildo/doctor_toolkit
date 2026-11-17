---
type: exe
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [100]
sources: ["docs/analysis/resources-in-memory.md §2", "docs/analysis/resources-in-memory.md §4", "research/file_io_sites.py", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "docs/analysis/game-bench.md §42"]
tags: [exe, files, streams, hooks]
updated: 2026-11-16
---
# Файловое устройство игры: чтение, переход, размер, закрытие

Адреса — виртуальные, издание PC 1.0.789.0. Файл, открытый файловой системой игры (`0x00AC0AD0`), дальше обслуживают пять функций над записью `{дескриптор, ?, позиция: u64 (+8)}`. Через них читаются все потоки `.str`: на стенде 1 262 чтения, 155,6 МБ за загрузку Гаваны — все из `0x00ABEC29`. Модуль связи отвечает за эти вызовы сам для потоков, которые он собирает в памяти.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00D9B0C4` | данные: таблица методов драйвера файлов: открыть `0x00AC0AD0`, закрыть (`+4`), прочитать (`+8`), записать `0x00AC2650` (`+0xC`), перейти (`+0x10`), размер (`+0x14`), обрезать (`+0x18`); других ссылок на эти функции нет | resources-in-memory.md §2 |
| `0x00ABEBE0` | закрыть файл устройства: `CloseHandle(запись[0])`, запись возвращается распределителю `*0x0113D190` | resources-in-memory.md §2 |
| `0x00ABEBE8` | `call [0x00D55264]` (`CloseHandle`) в закрытии; модуль связи заменяет: забыть дескриптор собранного потока | resources-in-memory.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABEC10` | прочитать: `ReadFile(дескриптор, буфер, длина, &прочитано, NULL)` синхронно; позиция записи растёт на прочитанное | resources-in-memory.md §2 |
| `0x00ABEC40` | перейти: ничего, если позиция записи уже та; иначе `SetFilePointerEx(…, FILE_BEGIN)`, при отказе `SetFilePointer(…, 0, NULL, FILE_CURRENT)` | resources-in-memory.md §2 |
| `0x00ABEC64` | `call [0x00D55124]` (`SetFilePointerEx`) в переходе; модуль связи заменяет | resources-in-memory.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABEC78` | `call [0x00D5525C]` (`SetFilePointer`) в переходе после отказа; модуль связи заменяет | resources-in-memory.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABECA0` | размер: `GetFileSizeEx`, при отказе 0 | resources-in-memory.md §2 |
| `0x00ABECAE` | `call [0x00D55128]` (`GetFileSizeEx`) в размере; модуль связи заменяет | resources-in-memory.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABECD0` | обрезать файл (`SetFilePointerEx` через `EDI` в `0x00ABECE1`, `SetEndOfFile` в `0x00ABECF3`): только файлы на запись | resources-in-memory.md §2 |
| `0x00D55124` | данные: слот импорта `SetFilePointerEx` (ячейка поиска `0x00E4E6BC`, имя `0x00E4EF7A`) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00D55128` | данные: слот импорта `GetFileSizeEx` (ячейка поиска `0x00E4E6C0`, имя `0x00E4EF8E`) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00D5525C` | данные: слот импорта `SetFilePointer` (ячейка поиска `0x00E4E7F4`, имя `0x00E4EA9E`) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00D55264` | данные: слот импорта `CloseHandle` (ячейка поиска `0x00E4E7FC`, имя `0x00E4EA82`); 63 места в коде, заменяется одно | research/file_io_sites.py; crates/doctor-toolkit-live/src/edition.rs |

## Что неизвестно

- Второе слово записи (`+4`); запись заводит открытие `0x00AC0AD0` (не разбиралось).
- Читается ли что-нибудь из потоков через класс файла `0x00ABD6E0` или библиотеку C: на стенде — нет.

## Подробнее

- [Подмена ресурсов в памяти](../../analysis/resources-in-memory.md) §2, §4
- [Перехват чтения файлов](file-read-hooks.md), [перехват открытия файлов](file-open-hooks.md)
- [Файлы: как игра открывает и читает](../engine/file-system.md)
