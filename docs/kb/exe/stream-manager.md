---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 48, 45, 32, 84, 99]
sources: ["docs/analysis/live-link.md §4", "docs/analysis/live-link.md §12.3", "docs/analysis/live-link.md §13.3", "docs/analysis/new-resources.md §3.1", "docs/analysis/new-resources.md §3.2", "docs/analysis/new-resources.md §3.3", "docs/analysis/launch-flags.md §3.1", "docs/formats/toc.md §3.2", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "godfather2.exe 0x00401F40, 0x004A82B0", "docs/formats/wgi.md §4", "docs/formats/simgroup.md §1.1", "docs/analysis/crash-reports.md §5", "docs/analysis/game-bench.md §37"]
tags: [exe, streams, loading]
updated: 2026-11-16
---
# Менеджер потоков: запрос, запись, выгрузка

Адреса — виртуальные, издание PC 1.0.789.0. `StreamManager` держит список загруженных потоков и записи по id. Запрос потока по имени ищет его в загруженных TOC или грузит файл `<имя>.str`; зависимости TOC грузятся сами. Классы SimGroup `LoadStream`/`UnloadStream` зовут те же функции.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0122337C` | данные: `StreamManager` (указатель); `+0x6C` список загруженных потоков, `+0x84` — поток, объекты которого создаются сейчас | live-link.md §4; crates/doctor-toolkit-live/src/addr.rs |
| `0x00405F40` | по ресурсу находит запись потока в списке `+0x6C` и возвращает её id `+0x1C` | live-link.md §4; live-link.md §11.5 |
| `0x00403A50` | запрос потока: `u32 Request(this, имя, приоритет, флаги, u32*, u32)`, `thiscall`, `RET 0x14`; для имени без `.str` дописывает `.str`; нет в TOC — грузит файл по имени | live-link.md §13.3; new-resources.md §3.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00403C36` | место возврата запроса потока (`RET 0x14`) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00403C40` | загрузка записи TOC: предпосылки, сам поток, дочерние потоки | new-resources.md §3.1; toc.md §3.2 |
| `0x00404590` | запрос самого потока; смотрит флаги записи TOC в памяти (`+0xA`): биты `0x8`, `0x10`, `0x4`. Здесь отказ дочернему потоку с битом `0x4` (этап 84); без этого бита дочерний поток приходит с родителем | new-resources.md §3.1 |
| `0x004A8100` | чтение флагов записи TOC в памяти (`+0xA`) | new-resources.md §3.1 |
| `0x004061F0` | проверка «parented stream»: такой поток не грузится сам по себе | new-resources.md §3.1 |
| `0x004040D0` | загрузка `<имя>.toc` (до 32 загруженных TOC: массив `+0xA4`, счётчик `+0x124`) | new-resources.md §3.1 |
| `0x00405900` | `u32 IdOf(this, имя, 0)`, `thiscall`, `RET 8`: id потока по имени | live-link.md §13.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x004025A0` | `Record* RecordOf(this, id)`, `thiscall`, `RET 4`: корзины `+0x50`, их число `+0x54` (`id % число`); узел корзины — `{id, запись, следующий}`. Отчёт о падении обходит корзины и перечисляет записи не в состоянии 3; у запрошенных и ещё не читаемых потоков состояние 0, у читаемого — 1 (стенд, game-bench.md §37) | live-link.md §13.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00404C40` | `Unload(this, Record*)`, `thiscall`, `RET 4` (→ `0x004048C0`) | live-link.md §13.3; live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x004048C0` | выгрузка потока (из `0x00404C40`) | live-link.md §12.3 |
| `0x008139D0` | обработчик класса `LoadStream` | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x008139E7` | начало места `LoadStream`, где зовётся `0x00403A50(имя, приоритет, 0, 0, 0)` | live-link.md §13.3; crates/doctor-toolkit-live/src/edition.rs |
| `0x00813A1F` | конец этого места (`0x008139E7`–`0x00813A1F`) | live-link.md §13.3 |
| `0x00E44690` | данные: приоритет `LoadStream` по умолчанию, 32768 (`f32`) | live-link.md §13.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00813BC0` | обработчик класса `UnloadStream` | live-link.md §12.3 |
| `0x00813BCB` | начало места `UnloadStream`: `0x00405900` → `0x004025A0` → `0x00404C40` | live-link.md §13.3; crates/doctor-toolkit-live/src/edition.rs |
| `0x00813C01` | конец этого места (`0x00813BCB`–`0x00813C01`) | live-link.md §13.3 |
| `0x00813BFB` | место вызова `Unload` в `UnloadStream` | crates/doctor-toolkit-live/src/addr.rs |
| `0x00813C10` | функция, которая берёт id и запись потока по имени (как `UnloadStream`) | crates/doctor-toolkit-live-asi/src/state.rs |
| `0x00813C3E` | место проверки состояния записи потока (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x00813C4A` | место, где состояние 3 записи (`+0x24`) означает «загружен» | live-link.md §13.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x008142F0` | менеджер принудительной загрузки: ставит биты `0x8`/`0x10` записи TOC (зовёт `0x00406230`); если хэша нет в TOC — включает или выключает SimGroup с таким хэшем группы (`0x00447AB0`) | new-resources.md §3.1; simgroup.md §1.1 |
| `0x00406230` | `(хэш, вкл)`: поставить бит принудительной загрузки | new-resources.md §3.1 |
| `0x00406280` | снять бит принудительной загрузки | new-resources.md §3.1 |
| `0x0045E100` | поток по GUID сущности-владельца (объект класса `0xAE986323` берёт поток по GUID самого объекта) | new-resources.md §3.2 |
| `0x00401F40` | свёртка GUID в хэш записи TOC, `thiscall(результат, GUID)`: `((a*33 + b)*33 + c)*33 + d`; 4 вызова: `0x00403A50` (запрос потока), `0x0045E100`, `0x006AEF20`, `0x008F0520` | new-resources.md §3.2; toc.md §3.5 |
| `0x004A82B0` | поиск записи TOC по GUID: та же свёртка и двоичный поиск `0x004A8180`; прямых ссылок на функцию в exe не найдено | toc.md §3.5; gf2-pc-verification.md §4 |
| `0x004A8180` | двоичный поиск записи TOC по хэшу | new-resources.md §3.4; toc.md §3.5 |
| `0x0088BB70` | использование `data_size` записи для бюджета памяти потоковой загрузки: это загрузка раздела [мировой сетки](world-grid.md) | toc.md §3.5; wgi.md §4 |

## Что неизвестно

Два запроса потоков подряд роняют игру (см. заметку об известных падениях).

## Подробнее

- [Живая связь](../../analysis/live-link.md) §4, §12.3, §13.3
- [Новые ресурсы и потоки](../../analysis/new-resources.md) §3
- [Формат TOC](../../formats/toc.md)
- [Формат потока](../../formats/stream.md)
