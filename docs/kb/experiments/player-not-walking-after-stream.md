---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 33, 45]
sources: ["docs/analysis/game-bench.md §20.1", "docs/analysis/live-link.md §5.1", "docs/analysis/game-bench.md §17"]
tags: [input, controller-port, live-module, stream]
updated: 2026-11-16
---
# Игрок не ходит после -stream: причина и исправление

**Симптом** (этап 33): после старта `-stream` игрок не ходит и камера не поворачивается мышью;
`Escape` открывает меню паузы. После загрузки сохранения всё работает.

**Причина** (этап 45, сравнение памяти двух сеансов — через сохранение и `-stream`): без меню игра
создаёт игрока до того, как менеджер контроллера получит порт клавиатуры (15). Различаются ровно два
места: `Player+0x2498` (15 / 0) и стек карт действий `*0x01223464` (карта `basic_aiming_player` на
порту 15 / на порту 0); флаги игрока одинаковы. Пауза работает, потому что меню опрашивает все порты.

Не помогают: сообщение `iMsgPlayerControlEnable`, флаг `-inputwithoutfocus`, запись одного порта
(`Player+0x2498 = 15`) — нужна и карта.

**Исправление** — модуль связи (`fix_input`, по умолчанию включено): порт в `Player+0x2498`, байт
«порт занят», перенос карт на порт 15 функциями игры `0x00464960`/`0x00464A10`.

| Прогон | Результат |
|---|---|
| `-stream`, модуль без исправления | `w` 4 с, `Up`, `d`, `Left`, мышь — позиция и камера не меняются |
| `-stream` + `fix_input` | журнал: `player … read input port 0, given the controller's port 15 and moved action maps [basic_aiming_player]`; `w` 4 с — 24,9 м, мышь повернула камеру на 34° |
| `play --stream gf2/cuba --resolution 1280x720` без `--live` | «live: module installed for this launch so that the player can move…»; игрок ушёл за ворота; после выхода `cmd.txt` и модуль убраны |
| через сохранение | порты совпадают — модуль ничего не меняет |

«Играть» и `play --stream` ставят модуль на запуск сами (запись по сети выключена), если в папке игры
есть ASI-загрузчик; без него — предупреждение.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §17, §20.1
- [Живая связь](../../analysis/live-link.md) §5.1
- [Быстрый вход `-stream`](quick-entry-stream-flag.md)
