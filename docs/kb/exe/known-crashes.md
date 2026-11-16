---
type: exe
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 32, 14, 28, 63]
sources: ["docs/analysis/launch-flags.md §3.4", "docs/analysis/game-bench.md §10.3", "docs/analysis/game-bench.md §14", "docs/analysis/game-bench.md §16", "docs/analysis/game-bench.md §18", "docs/analysis/game-bench.md §30", "docs/levels.md §6", "docs/scripts.md", "docs/formats/mission-logic.md §3.5", "docs/formats/resource-types.md §5", "docs/analysis/queens.md §7", "docs/analysis/live-link.md §2"]
tags: [exe, crashes, bench]
updated: 2026-11-16
---
# Известные падения: адреса

Адреса — виртуальные, издание PC 1.0.789.0. Адреса, по которым игра падала на стенде, и условия. Адрес `0x6E756F7B` лежит вне образа `exe`; причина этого падения не выяснена.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0042138B` | падение при `-stream` с двумя и более потоками: регистрация ресурса без описания типа, чтение `[x+0x18]+0xC` при `x+0x18 = 0` | launch-flags.md §3.4; game-bench.md §16 |
| `0x00890CEB` | падение при `-stream` с одним дочерним потоком (`cuba\local`), 3 из 3 | launch-flags.md §3.4; game-bench.md §16 |
| `0x008CAE17` | чтение по нулевому указателю `+0x14`: объект `Destructible` с metamodel без состояний `DestructibleState` | game-bench.md §10.3; levels.md §6 |
| `0x004420F3` | падение при загрузке меню: объекты с переменными в `gf2/frontend`, нет менеджера переменных | game-bench.md §14; mission-logic.md §3.5 |
| `0x00480E03` | падение: объект со скриптом Lua в главном меню, где нет состояния Lua | game-bench.md §14; scripts.md |
| `0x0040C278` | чтение по адресу `0x14`: затемнение экрана при телепорте вне кадра (буфер команд кадра пуст) | live-link.md §2; game-bench.md §18 |
| `0x00BAADFA` | запись за границу буфера: текстуры APX `popup` и `cursor` с полной цепочкой мип-уровней | resource-types.md §5 |
| `0x6E756F7B` | падение при входе в старт Миссии 3, запущенной сообщением `cp_325_start_now` со старта `-stream` (адрес вне образа) | queens.md §7; game-bench.md §30 |

## Что неизвестно

Падение `0x6E756F7B` при входе в стартовый триггер Миссии 3 (и без мода) не разобрано.

## Подробнее

- [Флаги запуска](../../analysis/launch-flags.md) §3.4
- [Стенд](../../analysis/game-bench.md) §10.3, §14, §16, §18, §30
- [Уровни](../../levels.md) §6
- [Нью-Йорк](../../analysis/queens.md) §7
