---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 73, 98, 83]
sources: ["docs/formats/resource-types.md §8", "data/gf2/enums.toml", "research/enum_values.py", "docs/formats/audio.md §13", "research/speech_survey.py"]
tags: [question, resource-type, format]
updated: 2026-11-16
---
# Формат ресурса `auc` не разобран

Решено на этапе 83: `auc` — конфигурация звука, оба ресурса читаются и пишутся побайтно.

## Ответ

- Заголовок 0x20 (версия 11), затем 49 категорий микшера по 100 байт (имя, второе имя, родитель), 6 пулов банков по 40 байт, 3 «дака» по 176 байт (имя и имя микса).
- Категории — дерево из корней `DIALOG`, `MUSIC`, `SFX`; звуки и миксы называют категорию хэшем `sdbm` имени.
- Загрузчик `0x005E35E0` (из `0x00456460`); банк `sbk` не загружается, пока `auc` не загружен (`0x004562F0`).
- Тулкит: `resource/auc.rs`, команда `audio-config`, новая категория — `--add-category`.

## Что осталось

- [Новая категория звука и правка микса в игре не проверены](new-audio-category-in-game.md).

## Связанное

- [Тип ресурса `auc`](../resources/auc.md)
- [Тип ресурса `mix`](../resources/mix.md)
- [docs/formats/audio.md](../../formats/audio.md) §13
