---
type: question
status: unknown
confidence: data
editions: ["1.0.789.0"]
stages: [72]
sources: ["docs/navigation.md §7", "docs/formats/logic-types.md §1", "docs/formats/characters.md §8", "docs/formats/resource-types.md §1"]
tags: [question, hash, names]
updated: 2026-11-16
---
# Функции хэшей имён текстур, переменных и id ресурсов

Не найдены: функция хэша имени текстуры (`tg4h`), хэш имени переменной `variabledictionary` (не SDBM, FNV-1a, FNV-1, djb2 и не CRC-32), функция id ресурсов, кроме `smb` и `sbk` (SDBM пути сборки). Id мешей — SDBM строки с неизвестным префиксом и четырьмя символами после LOD; для правки не нужна.

## Следующий шаг

Найти в exe места, где эти хэши считаются во время выполнения (поиск переменной по имени, текстуры по имени), и восстановить функцию.

## Связанное

- [Хэши имён и сжатие](../engine/hashing.md)
- [Тип ресурса `tg4h`](../resources/tg4h.md)
- [Тип ресурса `variabledictionary`](../resources/variabledictionary.md)
- [docs/navigation.md](../../navigation.md) §7
- [docs/formats/logic-types.md](../../formats/logic-types.md) §1, §2
