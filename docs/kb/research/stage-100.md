---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [100]
sources: ["docs/analysis/resources-in-memory.md", "docs/analysis/game-bench.md §42", "docs/analysis/live-link.md §17", "crates/doctor-toolkit-live/src/resources.rs", "research/file_io_sites.py"]
tags: [research, mods, streams]
updated: 2026-11-16
---
# Этап 100: Подмена ресурсов в памяти

- Состояние: сделан (2026-11-16); переключатель выключен по умолчанию.
- Зависит от: [этап 72](stage-72.md), [этап 84](stage-84.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Модуль связи собирает поток в памяти из файла игры и заплатки ресурсов и отвечает на чтение игры — [описание](../../analysis/resources-in-memory.md), [потоки](../engine/stream-loading.md), [модуль](../engine/live-module.md).
- Найдено [файловое устройство игры](../exe/file-device.md): через него читаются все потоки; четыре новых места перехвата и существующий перехват `ReadFile`.
- Таблица загруженных ресурсов: указатель, запись и хэш — [загрузчик ресурсов](../exe/resource-loader.md); какие типы остаются в памяти как в потоке — [образ ресурса после загрузки](../engine/resource-images.md).
- «Играть» и «Играть с модами» получили переключатель (выключен); подмена файлов этапа 46 остаётся основным путём; TOC, новые потоки и файлы архивов идут ею же.
- На стенде: три города, замены того же и другого размера, добавленные ресурсы, новый поток, корневые `*_global`, три мода из менеджера, серия замеров — [опыт](../experiments/resources-in-memory.md).

## Что осталось

- [Windows и большие моды](../questions/resources-in-memory-on-windows.md) — до этого переключатель выключен.
- [Слияние SimGroup по объектам](../questions/simgroup-merge-by-objects.md): оставлено место, не сделано.
- [Вес зоны после подмены](../questions/stream-weight-after-substitution.md): равенство чисел проверено, вытеснение зон при большом росте — нет.
- [Указатели выше 2 ГБ с LAA](../questions/laa-pointers-above-2gb.md): этап вопрос не закрыл.

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
