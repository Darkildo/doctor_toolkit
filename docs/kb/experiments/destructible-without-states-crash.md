---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 14, 21]
sources: ["docs/analysis/game-bench.md §10.3", "docs/analysis/game-bench.md §10.5"]
tags: [crash, destructible, metamodel, simgroup]
updated: 2026-11-16
---
# Destructible с metamodel без состояний роняет игру

Прогоны C, D, E этапа 14 в группе `cp_010_hotel_interior_zonestream` (пепельница — объект 77,
`Entity`, модель `3e3f49de`).

- **Прогоны C и D** упали на 573-й секунде, в конце вступительной сцены (когда создаются объекты
  этапа): `Unhandled page fault on read access to 00000014 at address 008CAE17`.
- Общее у C и D: в потоке `s14set` новый объект был копией люстры (`Destructible`) с моделью
  пепельницы (`Entity`). В C так же был переведён `Destructible` 97.
- **Прогон E** — объект `s14set` заменён копией самой пепельницы (`level add … --template-stream
  <зона> --template-resource 98`) — прошёл: управление на 567-й секунде.

Вывод: объект класса `Destructible` с metamodel без состояний `DestructibleState` роняет игру
(чтение по нулевому указателю `+0x14` в `0x008CAE17`) при создании объектов этапа.

Что из этого следует: с этапа 21 `simgroup-set --model`, `level add --model` и GUI такую замену
отказывают (`level::compat`); правило выведено по корпусу — `Destructible` с флагами броска 64 и
подсветкой интерьера, как у люстры, не бывает без состояний `DestructibleState`.

Попутно: падение кода игры Wine показывает окном отладчика, процесс остаётся живым, и сценарий ждал
`hud` до остановки; теперь `await` проверяет `wine.log`.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §10.3, §10.5
- [Уровни](../../levels.md) §6
- [Падения игры на стенде](game-crashes.md)
