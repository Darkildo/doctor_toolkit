---
type: question
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72]
sources: ["docs/navigation.md §7"]
tags: [question, tooling, index]
updated: 2026-11-16
---
# Ссылки между ресурсами, которые индекс не извлекает

Индекс игры не извлекает ссылки: `materialswaplistresource` (хэши материалов, раскладка не разобрана), `vfx` → текстуры, `trinity_seq_master` → актёры, `wgi`, `hkx` → материалы Havok, `lhr` → потоки текста. Индекс также не учитывает неприменённые правки рабочего пространства.

## Следующий шаг

Добавлять извлечение ссылок по мере разбора форматов (`vfx`, `wgi`, `materialswaplistresource`).

## Связанное

- [Тип ресурса `materialswaplistresource`](../resources/materialswaplistresource.md)
- [Тип ресурса `vfx`](../resources/vfx.md)
- [Тип ресурса `wgi`](../resources/wgi.md)
- [docs/navigation.md](../../navigation.md) §7
