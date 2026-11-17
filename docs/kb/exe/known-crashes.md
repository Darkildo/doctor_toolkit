---
type: exe
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 32, 14, 28, 63, 99, 74, 77]
sources: ["docs/analysis/story.md §9", "docs/analysis/game-bench.md §40", "docs/analysis/launch-flags.md §3.4", "docs/analysis/game-bench.md §10.3", "docs/analysis/game-bench.md §14", "docs/analysis/game-bench.md §16", "docs/analysis/game-bench.md §18", "docs/analysis/game-bench.md §30", "docs/levels.md §6", "docs/scripts.md", "docs/formats/mission-logic.md §3.5", "docs/formats/resource-types.md §5", "docs/analysis/queens.md §7", "docs/analysis/live-link.md §2", "docs/analysis/game-bench.md §37", "docs/analysis/crash-reports.md §9", "docs/analysis/game-bench.md §35", "docs/analysis/lua-api.md §4"]
tags: [exe, crashes, bench]
updated: 2026-11-16
---
# Известные падения: адреса

Адреса — виртуальные, издание PC 1.0.789.0. Адреса, по которым игра падала на стенде, и условия. Адрес `0x6E756F7B` лежит вне образа `exe`: это возврат по адресу, затёртому текстом слишком длинного пути к потоку (этап 77, [очередь чтения файлов](async-file-io.md)). С этапа 99 падение оставляет отчёт и минидамп в `_doctor_toolkit/crashes/` ([перехват падений](crash-handler.md)), а строки этой таблицы попадают в отчёт как имена адресов.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0042138B` | падение при `-stream` с двумя и более потоками: регистрация ресурса без описания типа, чтение `[x+0x18]+0xC` при `x+0x18 = 0` | launch-flags.md §3.4; game-bench.md §16 |
| `0x00890CEB` | падение при `-stream` с одним дочерним потоком (`cuba\local`), 3 из 3 | launch-flags.md §3.4; game-bench.md §16 |
| `0x008CAE17` | чтение по нулевому указателю `+0x14`: объект `Destructible` с metamodel без состояний `DestructibleState` | game-bench.md §10.3; levels.md §6 |
| `0x004420F3` | падение при загрузке меню: объекты с переменными в `gf2/frontend`, нет менеджера переменных | game-bench.md §14; mission-logic.md §3.5 |
| `0x00480E03` | падение: объект со скриптом Lua в главном меню, где нет состояния Lua | game-bench.md §14; scripts.md |
| `0x0040C278` | чтение по адресу `0x14`: затемнение экрана при телепорте вне кадра (буфер команд кадра пуст) | live-link.md §2; game-bench.md §18 |
| `0x00BAADFA` | запись за границу буфера: текстуры APX `popup` и `cursor` с полной цепочкой мип-уровней | resource-types.md §5 |
| `0x6E756F7B` | падение при загрузке потока, чей полный путь длиннее 255 символов (папка игры слишком длинная): адрес возврата `0x00AC37C0` затёрт текстом пути; на стенде — буквы `poun` (`0x6E756F70`) из `…m03_carmine_compound_exp.str` у старта Миссии 3; исполнение останавливается через несколько байт (`0x6E756F77`, `0x6E756F7B`) или на вызове `0xFFFFFFFF` | analysis/story.md §9; game-bench.md §40; game-bench.md §30; game-bench.md §37 |
| `0x6E756F70` | адрес, куда возвращается игра при длинном пути папки: текст `poun` слова «compound» в пути потока чекпоинта 326 | analysis/story.md §9 |
| `0x00469F85` | чтение по нулю в `Group:AddMember` (Lua): ссылка `Base` пуста или GUID не найден — `0x46B980` вернула 0, результат разыменован без проверки; то же по коду у `RemoveMember`, `IsMember`, `GroupManager:RemoveFromAllGroups`. `Base` от строки в этой сборке объектов игры не находит, так что это любой такой вызов | game-bench.md §35; lua-api.md §4 |

## Что неизвестно

Причина падения `0x6E756F7B` установлена — см. [вопрос](../questions/crash-6e756f7b.md); открытых причин в таблице нет, кроме помеченных в строках.

## Подробнее

- [Перехват падений](../../analysis/crash-reports.md) §9
- [Сюжет и миссии: разбор](../../analysis/story.md) §9

- [Флаги запуска](../../analysis/launch-flags.md) §3.4
- [Стенд](../../analysis/game-bench.md) §10.3, §14, §16, §18, §30
- [Уровни](../../levels.md) §6
- [Нью-Йорк](../../analysis/queens.md) §7
- [Lua-API движка](../../analysis/lua-api.md) §4
