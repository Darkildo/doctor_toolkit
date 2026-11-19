---
type: question
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 85]
sources: ["docs/formats/resource-types.md §8", "docs/formats/bricknmortarres.md"]
tags: [question, resource-type, format]
updated: 2026-11-18
---
# Формат ресурса `bricknmortarres` не разобран

Разобран 2026-11-18: список заведений города (записи по 0x70 байт и имена эффектов), загрузчик `0x006B1000`, запись побайтно на всех 8 ресурсах; команда `venue`. Подробно — [формат](../../formats/bricknmortarres.md).

Новое заведение требует записи: `Building` с номером, которого нет в таблице, роняет игру (`0x006B5B87`); с записью — грузится и переходит к игроку (проверено на стенде).

## Связанное

- [Формат `bricknmortarres`](../../formats/bricknmortarres.md)
- [Тип ресурса `bricknmortarres`](../resources/bricknmortarres.md)
- [docs/formats/resource-types.md](../../formats/resource-types.md) §8
