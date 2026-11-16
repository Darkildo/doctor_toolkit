---
type: question
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72, 84]
sources: ["docs/formats/simgroup.md §1.1", "crates/doctor-toolkit/tests/corpus.rs"]
tags: [question, simgroup, hash]
updated: 2026-11-16
---
# Из чего считается хэш группы SimGroup (поле 0x10)

Решено на этапе 84: это **хэш потока группы в TOC** (свёртка GUID потока). Совпадает у 1 999 SimGroup из 2 006; у семи корневых `*_global` — 0 (корпусный тест `simgroup_group_hash_is_the_stream_hash`).

В exe поле — ключ группы у менеджера SimGroup: загрузчик `0x00448140` ищет его в таблице «хэш, флаги» (`0x00447890`), `0x00447070` по ней решает, активна ли группа, а записи добавляет `0x00447AB0` из действия `EnableDisableStreams` (`0x008142F0`) — когда хэша действия нет в загруженных TOC, оно включает и выключает группу, а не поток.

Тулкит ставит новым группам хэш их потока. До этапа 84 он ставил 0, и такие группы работали: для загрузки вместе с потоком поле не важно.

## Следующий шаг

Вопрос закрыт. Не проверено в игре: `EnableDisableStreams` с хэшем, которого нет в TOC (включение группы без потока).

## Связанное

- [Тип ресурса `simgroup`](../resources/simgroup.md)
- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md)
- [Потоки `.str` и их загрузка](../engine/stream-loading.md)
- [Формат SimGroup](../../formats/simgroup.md) §1.1
