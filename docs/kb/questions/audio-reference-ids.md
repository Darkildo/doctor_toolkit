---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [73, 83]
sources: ["data/gf2/attr_types.toml", "research/attr_types.py", "data/gf2/emitter_names.txt"]
tags: [question, simgroup, audio, types]
updated: 2026-11-16
---
# Ссылки на звук в атрибутах: id не найдены среди ресурсов

У 34 атрибутов-ссылок на ресурс (`Door` 19–21 — `CMD_SetOpenAudio`, `CMD_SetCloseAudio`, `CMD_SetLockedAudio`; `CarAudio` 3–6, 9, 11; `SimpleCarAudio`; `PlaySound` 3–4; `PlayAudioStream` 3; `AmbientAudioConfiguration`; `Item` 25–26; `TrinityGameSequence` 7) значение — id, повторённый четыре раза, но ресурса с таким id в потоках игры нет. Поэтому они остались без типа `Res<…>`. С хэшами 386 известных имён эмиттеров `.emx` (SDBM, CRC-32, FNV-1a, с расширением и без) эти id не совпали.

## Следующий шаг

Сравнить id с шаблонами всех 905 эмиттеров банков `sbk` (`doctor-toolkit sbk-info`), с SDBM полных путей сборки (`x:\assets\…\*.emx`, такие строки есть у `Gun` 103 и 105) и с id потокового звука в архивах `.viv`; для `TrinityGameSequence` 7 — с SDBM пути XML ролика.

Запланировано: [Этап 83](../research/stage-83.md).

## Связанное

- [Звук](../engine/audio.md)
- [Класс `Door`](../classes/Door.md)
- [Класс `PlaySound`](../classes/PlaySound.md)
- [Этап 73](../research/stage-73.md)
