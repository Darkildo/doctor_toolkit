---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 18]
sources: ["docs/formats/resource-types.md §4.1", "docs/formats/havok.md §1", "docs/formats/havok.md §2", "docs/formats/treemodel.md §4"]
tags: [exe, formats, strings, havok]
updated: 2026-11-16
---
# Прочие загрузчики форматов: строки, Havok, деревья

Адреса — виртуальные, издание PC 1.0.789.0. Отдельные функции `exe`, по которым сверены форматы локализации `lh2`, коллизии Havok (MOPP) и моделей деревьев.

| Адрес | Что это | Источник |
|---|---|---|
| `0x004DB060` | хэш записи строки: SDBM имени в нижнем регистре (`h = h × 65599 + байт`) | resource-types.md §4.1 |
| `0x00603330` | поиск строки (→ `0x006039D0`) | resource-types.md §4.1 |
| `0x006039D0` | поиск по всем загруженным таблицам, начиная с последней загруженной | resource-types.md §4.1 |
| `0x00605D00` | двоичный поиск хэша в таблице строк | resource-types.md §4.1 |
| `0x00603610` | форматирование строки: `$имя`, `#n`, `{x:…}` | resource-types.md §4.1 |
| `0x00C08290` | «доделка» `hkpMoppBvTreeShape`: ставит таблицы виртуальных методов и копирует `m_moppData` | havok.md §1 |
| `0x00C61BA0` | машина запроса MOPP: подготовка | havok.md §2 |
| `0x00C61050` | машина запроса MOPP: интерпретатор байт-кода | havok.md §2 |
| `0x00C31810` | `hkpMoppBvTreeShape::queryAabb`: обрезает ящик запроса пространством кода | havok.md §2 |
| `0x009D28D0` | загрузчик модели дерева | treemodel.md §4 |
| `0x009D1A40` | отрисовка дерева | treemodel.md §4 |
| `0x009D0D20` | выбор листа от поворота камеры (рысканье и тангаж) | treemodel.md §4 |

## Подробнее

- [Типы ресурсов](../../formats/resource-types.md) §4.1
- [Havok](../../formats/havok.md) §1, §2
- [Модели деревьев](../../formats/treemodel.md) §4
