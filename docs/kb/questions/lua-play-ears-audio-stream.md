---
type: question
status: unknown
confidence: guess
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/scripts.md §9"]
tags: [question, lua, audio]
updated: 2026-11-16
---
# `PlayEarsAudioStream` из своего скрипта не проверен в игре

Потоковый звук из Lua (`PlayEarsAudioStream`) в игре не проверялся: узел графа скриптов есть, но слышен ли поток из своего скрипта и какие аргументы нужны — не установлено.

## Следующий шаг

Разобрать функцию в exe (аргументы, результат), запустить на стенде с записью звука (`AUDIO_CAPTURE=1`).

Запланировано: [Этап 74: Lua-API движка](../research/stage-74.md).

## Связанное

- [Lua в игре](../engine/lua.md)
- [Звук](../engine/audio.md)
- [Свои скрипты Lua: где они запускаются и что нужно для этого](../experiments/lua-own-scripts.md)
- [docs/scripts.md](../../scripts.md) §9
