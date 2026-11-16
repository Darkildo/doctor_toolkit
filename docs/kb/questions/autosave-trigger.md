---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [76, 77]
sources: ["docs/formats/save.md §2, §4", "data/gf2/logic_schema.toml packet.AutoSave"]
tags: [question, saves, autosave]
updated: 2026-11-16
---
# Когда срабатывает автосохранение и кто шлёт сообщение объектам AutoSave

Известно: имя файла автосохранения собирает `0x008F2460`; менеджер сохранений (`0x008F2690`) слушает `iMsgManualSaveEnable` и `iMsgManualSaveDisable` (бит `0x80` поля `+0x10` запрещает ручное сохранение); в сохранении лежат переменные `autosave_favors`, `autosave_venues`, `autosave_family`, `autosave_hits`, `autosave_recruits` (в сохранении с 8 часами игры все равны 1). Класс [AutoSave](../classes/AutoSave.md) — пакет `41b65f6c`, атрибут 0 — сообщение-вход.

Не известно: какие события игры запускают автосохранение, что означают переменные `autosave_*` (догадка: разрешение автосохранения после события своего вида) и кто посылает сообщение объектам `AutoSave`.

## Следующий шаг

Найти обработчик класса `AutoSave` (строка `AutoSave` — `0x00D58DF8`) и вызовы сохранения с маской из менеджера `0x008F2690` (`0x008F2A60`); по графу логики — кто шлёт сообщение атрибута 0 объектам `AutoSave` и кто читает `autosave_*`. На стенде — журнал сообщений модуля связи вокруг надписи автосохранения.

## Связанное

- [Сохранения](../systems/saves.md)
- [Сюжет](../systems/story.md)
