---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72, 83, 97]
sources: ["docs/formats/cutscenes.md §8"]
tags: [question, cutscenes, audio]
updated: 2026-11-16
---
# Кто запускает дорожку `aud_*` ролика; поля `underscoreSpecialEvents`; миксы `igc_*.mix`

Не известно, кто запускает дорожку `aud_*` ролика (секвенсор?), что значат поля 2–5 записей `underscoreSpecialEvents` и что делают миксы `igc_*.mix` с громкостью речи и музыки.

Этап 83: формат `mix` разобран (каналы — уровни категорий звука, `docs/formats/audio.md` §14), миксы `igc_NN.mix` читаются командой `mix`; дорожки `aud_*` роликов — записи банков `smb` категории `IGC`. Кто запускает дорожку и что значат поля `underscoreSpecialEvents`, по-прежнему не известно.

## Следующий шаг

Проследить в exe запуск дорожки `aud_*` ролика; сравнить каналы `igc_NN.mix` с `global_default.mix` командой `mix`.

Запланировано: [Этап 97: Ролики](../research/stage-97.md).

## Связанное

- [Ролики](../systems/cutscenes.md)
- [Звук](../engine/audio.md)
- [Тип ресурса `mix`](../resources/mix.md)
- [docs/formats/cutscenes.md](../../formats/cutscenes.md) §8
