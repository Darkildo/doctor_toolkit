---
type: map
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72]
sources: []
tags: [map, engine]
updated: 2026-11-16
---
# Карта: движок

Подсистемы движка EARS в The Godfather II (PC 1.0.789.0). Полные списки: [подсистемы](../engine/index.md), [функции и адреса exe](../exe/index.md). Назад: [карта базы](../index.md).

## Ядро

- [Главный цикл](../engine/main-loop.md) — кадр и обновление, точки перехвата; адреса: [главный цикл](../exe/main-loop.md), [часы игры](../exe/game-clock.md).
- [Шина сообщений](../engine/message-bus.md) — имена и хэши сообщений, подписка, отправка; адреса: [шина](../exe/message-bus.md); список: [сообщения](../messages/index.md).
- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md) — объекты, пакеты атрибутов, регистрация классов; адреса: [реестр классов](../exe/simgroup-class-registry.md), [SimManager](../exe/sim-manager-objects.md), [Entity](../exe/entity-matrix.md).
- [Lua в игре](../engine/lua.md) — Lua 5.0, функции движка для скриптов; адреса: [среда Lua](../exe/lua-runtime.md), [ошибки Lua](../exe/lua-errors.md).
- [Хэши имён и сжатие](../engine/hashing.md) — SDBM, CRC-32, RefPack.
- [Флаги запуска и `cmd.txt`](../engine/launch-flags.md) — быстрый вход `-stream`; адреса: [флаги](../exe/launch-flags.md), [разбор `cmd.txt`](../exe/cmd-txt-parsing.md), [точка появления](../exe/player-spawn-guid.md).

## Данные и память

- [Потоки `.str` и их загрузка](../engine/stream-loading.md) — потоки, TOC, новые потоки; адреса: [менеджер потоков](../exe/stream-manager.md), [загрузчик ресурсов](../exe/resource-loader.md).
- [Файлы](../engine/file-system.md) — как игра открывает и читает файлы, архивы; адреса: [файловая система](../exe/game-filesystem.md), перехваты [открытия](../exe/file-open-hooks.md) и [чтения](../exe/file-read-hooks.md).
- [Зоны мира](../engine/zones.md) — мировая сетка `wgi`, выбор зон по положению игрока, `ZoneStreamEnable`, свой поток в сетке; адреса: [мировая сетка](../exe/world-grid.md); опыт: [новый поток в сетке](../experiments/world-grid-new-zone.md).
- [Память](../engine/memory.md) — политики памяти ресурсов, LAA.

## Картинка и звук

- [Рендер](../engine/render.md) — D3D9, поток рендера, шейдеры, цепочка кадра; адреса: [устройство D3D](../exe/d3d-device.md), [поток рендера](../exe/render-thread.md), [показ кадра](../exe/frame-present.md), [vsync](../exe/vsync-limiter.md), [текстуры](../exe/texture-table.md), [меши](../exe/mesh-buffers.md), [шейдеры](../exe/shader-table.md), [байткод шейдеров](../exe/shader-bytecode-offsets.md), [свет и тон](../exe/lighting-tone-lut.md), [камера](../exe/camera-views.md).
- [Время суток, свет и погода](../engine/time-of-day.md) — адреса: [время суток](../exe/time-of-day.md), [погода](../exe/weather.md).
- [Анимация](../engine/animation.md) — скелеты, клипы, граф; адреса: [клипы](../exe/anim-clips.md), [узлы графа](../exe/anim-graph-nodes.md).
- [Звук](../engine/audio.md) — банки, потоки, речь; адреса: [банки звука](../exe/audio-banks.md).
- [Видео](../engine/video.md) — ролики `.vp6`; адреса: [проигрыватель](../exe/video-player.md).
- [Интерфейс: APX, шрифты, строки](../engine/ui-apt.md).

## Мир

- [Физика: Havok и коллизии](../engine/physics.md) — адреса: [части коллизии](../exe/collision-parts.md).
- [Навигационный граф и укрытия](../engine/navigation-graph.md) — адреса: [граф](../exe/navigation-graph.md).
- [Ввод](../engine/input.md) — адреса: [порты ввода](../exe/input-ports.md), [игрок и телепорт](../exe/player-teleport.md).

## Модули тулкита в игре

- [Модуль `doctor_live` (живая связь)](../engine/live-module.md) — что читает и пишет в запущенной игре; адреса: [проверка издания](../exe/edition-check.md), [журнал игры](../exe/game-journal-print.md), [профиль](../exe/profile-hotspots.md).
- Состояние игры в памяти: [семьи и деньги](../exe/family-money.md), [команда](../exe/player-crew.md), [чекпоинты и переменные](../exe/missions-variables.md).
- Прочее в exe: [обработчики логических классов](../exe/logic-class-handlers.md), [диалоги](../exe/dialogues.md), [секвенсор роликов](../exe/cutscenes-sequencer.md), [загрузчики форматов](../exe/format-loaders.md), [отладочные средства](../exe/debug-leftovers.md), [известные падения](../exe/known-crashes.md).

## Что неизвестно

Открытые вопросы по движку — в [списке вопросов](../questions/index.md); ближайшие этапы — в [карте исследований](research.md).
