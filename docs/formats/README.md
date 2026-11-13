# Спецификация форматов движка EARS

Самодостаточный справочник форматов файлов движка EARS (EA Redwood Shores / Visceral):
The Godfather 1/2, Dead Space 1/2/3, Dante's Inferno, The Simpsons Game.

| Документ | Тема |
|---|---|
| `stream.md` | Stream (.str): заголовок, секции, чанки, RwStream/UStream, PlatformID |
| `toc.md` | StreamTOC: magic, Version8/10, allocation-таблицы |
| `geometry.md` | EARSGeom V33/V42/V50 + вершинные дескрипторы; запись мешей (§7) |
| `simgroup.md` | SimGroup (magic `SimG`): объекты уровня, пакеты атрибутов, трансформы |
| `metamodel.md` | MetaModel (magic `MMdl`): части, состояния, свойства |
| `logic-types.md` | variabledictionary, partsdb, graph, `covernodetable` (`0xDF14DA75`) и `treemodel` (`0xCDB9D42B`) |
| `lua.md` | Скрипты `lua`: формат чанка Lua 5.0 с числами float, компилятор, декомпилятор, правка |
| `mission-logic.md` | Где живёт логика миссий: SimGroup и сообщения, Lua-API движка, `tob`, `get`, `score` |
| `resource-types.md` | TG4/TXD/DFF/LH2/APX/Material/Entity, обзор остальных типов, шрифты `ffn`, скрипты `lua` |
| `rig.md` | Риги `rcb` (скелеты), связь меша со скелетом, Havok `hkx` |
| `characters.md` | Сборка персонажей из `partsdb`, шейдеры и материалы одежды, `mobface`, анимации (`bnk`) |
| `audio.md` | Звук: SNR/SNS, EA-XAS v1, банки `sbk`, архивы `.viv` (BIGF), импорт звука |
| `dialogues.md` | Диалоги F2F: деревья `dlg`, события речи `chv`, банки `smb`, субтитры в файлах речи `.snu`, редактор |
| `video.md` | Ролики `movies/*.vp6`: контейнер EA, VP6, звук EA-XA, проигрыватель игры, замена |
| `cutscenes.md` | Ролики на движке: `trinity_seq_master`, команды секвенсора, кривые камеры, актёры, клипы, субтитры `TOB1` в звуке |
| `compression-and-hash.md` | RefPack (полный алгоритм) + LZX + SDBM |

## Статус проверки

Разделы, сверенные с файлами The Godfather II (PC), помечены в тексте
**«Проверено на GF2 PC»**. Остальное получено без образцов данных и требует
проверки. Протокол сверки: `docs/analysis/gf2-pc-verification.md`.

| Раздел | Статус |
|---|---|
| UStream (`stream.md` §6) | Проверено на GF2 PC, переписан |
| StreamTOC v10 (`toc.md` §3) | Проверено на GF2 PC, переписан |
| RefPack (`compression-and-hash.md` §1) | Проверено на GF2 PC, исправлены диапазоны |
| SDBM для тегов типов | Проверено на GF2 PC |
| RwStream, TOC v8, LZX | Не проверено, образцов нет |
| TG4 (`resource-types.md` §1) | Проверено на GF2 PC, переписан |
| LH2 (`resource-types.md` §4) | Проверено на GF2 PC, переписан |
| APX (`resource-types.md` §5) | Проверено на GF2 PC, переписан |
| Геометрия и материалы (`geometry.md`) | Проверено на GF2 PC, переписан |
| Риги `rcb`, Havok `hkx` (`rig.md`) | Проверено на GF2 PC |
| Звук: `sbk`, SNR/SNS, `.viv` (`audio.md`) | Проверено на GF2 PC; импорт звука проверен в игре; `bnk` — не звук, а анимации (`characters.md` §6, все 6 115 клипов) |
| Видео `.vp6` (`video.md`) | Проверено на GF2 PC: 59 файлов, побайтная пересборка, декодер совпадает с ffmpeg; замена видна в игре |
| Шрифты `ffn` (`resource-types.md` §11), скрипты `lua` (`lua.md`) | Проверено на GF2 PC; 30 скриптов декомпилируются и компилируются обратно |
| SimGroup (`simgroup.md`) | Проверено на GF2 PC, переписан; побайтный round-trip |
| MetaModel (`metamodel.md`) | Проверено на GF2 PC (чтение) |
| variabledictionary (`logic-types.md` §1) | Проверено на GF2 PC; побайтный round-trip |
| partsdb (`logic-types.md` §2) | Проверено на GF2 PC; побайтный round-trip |
| graph (`logic-types.md` §3) | Проверено на GF2 PC частично |
| Анимации `bnk`, `mobface`, шейдеры (`characters.md`) | Проверено на GF2 PC: клипы схем 1 и 4 декодируются все; схема 3 нет |
| `covernodetable`, `treemodel` (`logic-types.md` §4, §5) | Только заголовок |
| Entity (`resource-types.md` §7) | Неверно, см. `metamodel.md` |

## Ключевые магические числа и константы

| Значение | Смысл |
|---|---|
| `0x9CBA7B28` | StreamTOC magic (проверено на GF2; ранее ошибочно `0x9CBAB728`) |
| `0x53696D47` | SimGroup magic (`SimG`) |
| `0x4D4D646C` | MetaModel/Entity magic (`MMdl`) |
| `0x61707866` / `0x75697866` | APX magic (`apxf` / `uixf`) |
| `0x53484F43` | UStream-блок «секция» (`SHOC`) |
| `0x53484452` | UStream-блок заголовка секции (`SHDR`) |
| `0x53444154` | UStream-блок сырых данных (`SDAT`) |
| `0x5270616B` | UStream-блок RefPack (`Rpak`) |
| `0x4C5A5844` | UStream-блок LZX (`LZXD`) |
| `0xB9F0B9EC` | флаг RefPack-сжатой секции Stream |
| `0x1802FFFF` | версия чанка RenderWare (`RwChunkHeaderInfo`) |
| `0x46494C4C` | чанк `FILL` (padding) |
| `rwID_TEXDICTIONARY` | тип ресурса TXD (18 символов) |
| `tg4h` / `tg4d` / `LH2` | типы ресурсов TG4/локализации |
| `0x57E0E057 0x10C0C010` | Havok packfile (`hkx`) |
| `TNOF` | шрифт `ffn` |
| `sbnk` / `sdat` | банк звуков `sbk` |
| `BIGF` | архив `.viv` |

## Порядок чтения/записи (общая модель)

1. Stream-файл: заголовок (count/platform) → «hardcoded data» → таблица секций.
2. Каждая секция: id/флаг сжатия/размеры; при RefPack-флаге — распаковать.
3. Распакованная секция — последовательность RenderWare-чанков (type/size/version),
   диспетчеризуемых по `ChunkType` в `RwChunkHandler`.
4. Ресурсы: TG4 (текстуры), TXD (словарь текстур), DFF (модель), EARSGeom
   (геометрия), SimGroup (объекты), LH2 (локализация), APX (SWF), Material,
   Entity (MetaModel).

## Статус полноты

Подтверждённые поля/алгоритмы описаны таблицами смещений и псевдокодом. Закрыты
ранее неизвестные участки: UStream-layout, полный граф SimGroup, layout
EARSMesh (V33/V42/V50), обёртка APX/SWF, потребитель LZX, формат Entity/MetaModel.

Осталось невыводимым без образцов данных:
- точная семантика безымянных полей заголовков (счётчики/размеры) в SimGroup и
  EARSGeom — читаются побайтно, имена полей не сохраняются;
- внутренности SWF — публичный формат Adobe (в APX хранится как сырой блок `apti`);
- байтовый порядок `guid128`/`matrix` в бинарных атрибутах SimGroup — определяется
  классами `MetaTypeDetail_*`, требует образца.
