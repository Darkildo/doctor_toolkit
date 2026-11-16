---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 96]
sources: ["docs/formats/havok.md §5"]
tags: [question, physics, havok]
updated: 2026-11-16
---
# Слова `EAMaterial` и поля тела 0x34–0x4C в `hkx`

У `EAMaterial` известен номер поверхности, остальные слова — нет. Поля тела 0x34–0x4C — кэш широкой фазы во время выполнения; у новых тел там нули.

## Следующий шаг

Найти чтение `EAMaterial` в exe (звук шагов, следы пуль по поверхности).

Запланировано: [Этап 96: Персонажи, модели, физика](../research/stage-96.md).

## Связанное

- [Физика: Havok и коллизии](../engine/physics.md)
- [Тип ресурса `hkx`](../resources/hkx.md)
- [docs/formats/havok.md](../../formats/havok.md) §5
