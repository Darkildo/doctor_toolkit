---
type: engine
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 34, 35, 45, 46, 47, 48, 49, 58]
sources: ["docs/analysis/live-link.md §1", "docs/analysis/live-link.md §2", "docs/analysis/live-link.md §6", "docs/analysis/live-link.md §7", "docs/analysis/live-link.md §8", "docs/analysis/live-link.md §9", "docs/analysis/live-link.md §11", "docs/analysis/live-link.md §12", "docs/analysis/live-link.md §13", "docs/analysis/live-link.md §14", "docs/analysis/live-link.md §15", "docs/analysis/live-link.md §16", "docs/analysis/performance.md §10"]
tags: [live-link, asi, protocol, hooks, edition-check]
updated: 2026-11-16
---
# Модуль в игре `doctor_live` (живая связь)

`doctor_live.asi` — DLL i686 на Rust (474 КБ) в `scripts/` папки игры; её грузит ASI-загрузчик игрока (Ultimate ASI Loader как `dinput8.dll`, под Wine — `WINEDLLOVERRIDES=dinput8=n,b`). Редактор и CLI говорят с ней по TCP на `127.0.0.1` (порт по умолчанию 47801).

## Что известно

- Проверка издания — до любого действия, в `DllMain`: `MZ`, `PE`, `TimeDateStamp = 0x4AC976E5`, `SizeOfImage = 0x00E32000` и байты 131 места кода (перехватываемые вызовы, начала функций, слоты таблиц, инструкции с адресами глобальных объектов). Несовпадение — модуль ничего не ставит и отвечает `Edition` на всё, кроме Hello.
- Данные игры — только в безопасных точках [главного цикла](main-loop.md): чтение — перехват вызова `0x0068B04B`; запись — подмена слота `+0x2C` таблицы `GameMainLoop` (`0x00D57754`).
- Читает: игрока (список `*0x012233A0`, игрок — `массив[0] − 0x1F30`, матрица `+0xD0`), камеру, игровое время (`*0x01205224`), [время суток](time-of-day.md), загруженные [потоки](stream-loading.md), объекты [SimGroup](simgroup-objects.md) (8 664 объекта — 7 мс), память (`Peek`).
- Пишет: телепорт (как `PlayerTeleportRelay`: `Player::Teleport` `0x0079E3C0`; не работает при бите 7 в `Player+0x24A0`), сообщение [шины](message-bus.md) всем или одному объекту по GUID, строка [Lua](lua.md), время суток и его скорость, загрузка и выгрузка потока по одному, деньги (`AddMoney` `0x008C09A0`), переменные сюжета, свободная камера (сеттер вида `0x00422D40` на `iMsgBeforeRender`), текстуры без перезапуска (тот же размер, формат и число уровней), перемещение и скрытие объектов (`Entity::SetMatrix`, `SetVisible`) с журналом и отменой.
- Только читает: команду игрока (`*0x0112989C`, 7 мест), чекпоинты (менеджер `*0x011299C4`), `WeatherManager` (в одиночной игре его нет).
- Журнал (этап 49): `OutputDebugStringA` (слот импорта `0x00D550C4`), `print` скриптов (указатель `0x0110A6C0`), ошибки Lua со стеком, сообщения шины с получателями. Кольцо на 4096 событий. Отладочный вывод выпущенной сборки почти весь вырезан (`Debug::Printf` — пустые `ret`).
- Отладка (этап 35): пауза, шаг, точки останова — на сообщении (до доставки), на тексте вывода, на ошибке Lua; не больше 64 правил; игра отпускается сама через `hold_s` (120 с, не больше 900). В местах `bus`, `print`, `lua` — только чтение.
- Подмена файлов (этап 46): 13 мест вызова `CreateFileA`/`CreateFileW`/`GetFileAttributesA` (см. [файлы](file-system.md)); папка игры не меняется. Исправление [ввода](input.md) после `-stream` (`fix_input`). Профиль (этап 58): темп цикла и показа кадра, `ReadFile`, ЦП потоков, таблица текстур, адресное пространство, выборка `EIP` двух потоков.
- Протокол: кадр `DTLV`, версия (u16, сейчас 7), вид, номер запроса, длина (до 32 МБ). Версии: 1 — Hello, Status, Player, Camera, Streams, Objects, Teleport, Send, Lua, Peek; 2 — Events, World; 3 — Break, Control, Debug, SetCamera, Texture; 4 — TimeOfDay, SetTime, Weather, StreamLoad, StreamUnload, StreamQueue, Money, SetMoney, Family, Mission, Variables, SetVariable; 5 — Files; 6 — Object, ObjectTransform, ObjectEnable, ObjectUndo, ObjectEdits, ObjectForget; 7 — Profile. Модуль отвечает в версии запроса; клиент спускается 7 → 1. Запросы к главному потоку ждут до 20 с.
- Ошибки: `BadRequest`, `Version`, `Edition`, `NotReady`, `Timeout`, `Denied` (`writes=0`), `Failed`, `Unsupported`.
- Настройка — `doctor_live.cfg` (токен, `writes`, `journal`, `fix_input`, `files_log`, `redirect=`); «Играть» ставит модуль на время запуска и убирает после. Файлы игры и сохранения модуль не пишет; его единственный файл — журнал.
- Уживается с модом игрока GF2ASI (тот перехватывает `0x00403A50` и другие функции, не места модуля); графический модуль `doctor_render` ставится вместе с ним в любом порядке (см. [рендер](render.md)).

## Ограничения

- Адреса привязаны к одной сборке. Двигаются только `Entity` и `Destructible` без родителя; игрок, люди и машины — отказ. Модели — только перезапуском уровня. Два запроса потоков подряд роняют игру — очередь из одного.
- Вызов `As` у произвольных объектов таблицы групп вешает игру; `VirtualQuery` на каждое чтение под Wine останавливал главный поток на минуты.
- Свободная камера: зоны грузятся по игроку — вдали мир пуст.

## Что неизвестно

- Отправитель сообщения шины; переход на чекпоинт и запись команды не проверены и не делаются.
- Причина «обломков» у `Destructible` после сдвига на метры за кадр; правка вершин модели на месте не проверена.

## Подробнее

- [Живая связь с запущенной игрой](../../analysis/live-link.md) §1–§16
- [Стенд](../../analysis/game-bench.md) — проверки модуля в игре
- Системы игры: [экономика](../systems/economy.md), [команда](../systems/crew.md), [сюжет](../systems/story.md)
