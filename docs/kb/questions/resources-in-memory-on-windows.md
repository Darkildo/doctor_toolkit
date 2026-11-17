---
type: question
status: unknown
confidence: bench
editions: ["1.0.789.0"]
stages: [100]
sources: ["docs/analysis/resources-in-memory.md §7", "docs/analysis/resources-in-memory.md §11", "docs/analysis/game-bench.md §42"]
tags: [question, mods, streams, windows]
updated: 2026-11-16
---
# Подмена ресурсов в памяти на Windows и с большими модами

Подмена ресурсов в памяти (этап 100) проверена только на стенде: Wine + DXVK под Xvfb, три города, минуты игры, тестовые заплатки и один мод с новой зоной. Поэтому она выключена по умолчанию. Можно ли включить её всем — неизвестно.

## Что известно

- Перехватываются только вызовы `KERNEL32` в файловом устройстве игры ([адреса](../exe/file-device.md)); неизменённые блоки модуль читает через дескриптор, который держит игра, своими `SetFilePointerEx` и `ReadFile`. Под Wine это работает; на Windows семантика та же по документации, прогона не было.
- Путь к заплатке под Wine — `Z:\…`, на Windows — обычный путь; длинные пути и не-ASCII в пути не проверялись.
- Ultimate ASI Loader с подменой файлов из папки `update` и GF2ASI на этом пути не проверялись (с подменой файлов этапа 46 — проверялись).

## Следующий шаг

1. Прогон на Windows: `play --mods --resources-in-memory --stream gf2/cuba` с модом, меняющим зоны; `live resources` — все потоки собраны, отказов нет; `live resource-check` против копии с `mods apply`.
2. Тот же прогон с GF2ASI и с папкой `update` загрузчика.
3. Долгая игра (час, езда по городу) с модом «Новый Нью-Йорк»: отчёты модуля падений, `live resources` в конце.
4. После этого — решить, включать ли переключатель по умолчанию.

## Связанное

- [Потоки и их загрузка](../engine/stream-loading.md)
- [Опыт на стенде](../experiments/resources-in-memory.md)
- [Модуль связи](../engine/live-module.md)
