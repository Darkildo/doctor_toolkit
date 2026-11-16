---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [83]
sources: ["docs/formats/audio.md §13", "docs/formats/audio.md §14", "docs/formats/audio.md §15", "docs/formats/dialogues.md §3.1", "docs/formats/dialogues.md §4", "docs/formats/dialogues.md §6", "docs/analysis/game-bench.md §34", "research/speech_survey.py"]
tags: [research, speech, audio]
updated: 2026-11-16
---
# Этап 83: Озвучка

- Состояние: сделан (2026-11-16).
- Зависит от: [этап 72](stage-72.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Форматы: [`smb`](../resources/smb.md), [`get`](../resources/get.md), [`auc`](../resources/auc.md), [`mix`](../resources/mix.md) читаются и пишутся побайтно (корпусные тесты); в [`chv`](../resources/chv.md) добавляются события и строки.
- По коду exe: поиск реплики по id, построение пути файла, обязательность записи `get`, порядок наборов речи говорящего, объект `SimpleChatterBox` — [адреса](../exe/speech.md).
- Новая голосовая реплика из записи одной операцией: команда `voice-add`, мастер редактора, мод формата 4 со звуком внутри.
- Стенд: [новая реплика с новым событием звучит, субтитр виден](../experiments/new-voice-line.md).

## Вопросы этапа

- [Новые события речи](../questions/new-speech-events.md) — решён.
- [Формат `auc`](../questions/resource-format-auc.md) — решён.
- [Какой `chv` говорит реплику](../questions/dialogue-which-chv-speaks.md) — правило найдено, редактор его не применяет.
- [Формат `mix`: смысл уровней](../questions/resource-format-mix.md) — структура есть, уровни названы по данным.
- Новые: [реплика персонажа и диалога на стенде](../questions/voice-line-of-a-character.md), [поля `smb` и `get`](../questions/speech-bank-and-event-fields.md), [новая категория и микс в игре](../questions/new-audio-category-in-game.md).

## Дальше

См. [карту исследований](../maps/research.md) и [как обновлять базу](../README.md).
