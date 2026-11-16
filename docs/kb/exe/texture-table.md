---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 35, 58]
sources: ["docs/analysis/live-link.md §12.3", "docs/analysis/live-link.md §16.1", "docs/analysis/renderer.md §6", "docs/analysis/renderer.md §10.7", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/render.rs"]
tags: [exe, textures, render]
updated: 2026-11-16
---
# Текстуры: таблица 4096 записей и загрузчик tg4

Адреса — виртуальные, издание PC 1.0.789.0. Все текстуры игры лежат в одной таблице на 4096 записей по 0x38 байт; «ручка» текстуры — индекс записи. Модуль подменяет содержимое текстуры без перезапуска той же командой буфера кадра, что и загрузчик `tg4`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x011A0F28` | данные: таблица текстур, 4096 записей × 0x38 байт: `+0` состояние, `+1` уровней, `+2`/`+4` ширина и высота, `+8` формат, `+0xC` флаги, `+0x14` `IDirect3DBaseTexture9*`, `+0x30` пул | live-link.md §12.3; renderer.md §6; crates/doctor-toolkit-live/src/addr.rs |
| `0x011A0F5F` | данные: конец диапазона абсолютных адресов полей первой записи (`0x011A0F28`–`0x011A0F5F`), на которые ссылаются 282 инструкции | renderer.md §6 |
| `0x011D8F28` | данные: конец таблицы; битовая карта занятости (64 × u64) сразу за ней | renderer.md §6 |
| `0x011D9128` | данные: номер первого слова битовой карты со свободным битом | renderer.md §6 |
| `0x00F15860` | данные: таблица форматов (индекс — поле `+8` записи текстуры) | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00606C80` | создать запись текстуры: номер из `0x00606B50` или заданный, затем `0x00618640`; нет места — `0xFFFFFFFF` | renderer.md §6; live-link.md §12.3 |
| `0x00606B50` | поиск свободной записи по битовой карте | renderer.md §6 |
| `0x006065A0` | освободить запись текстуры | renderer.md §6; live-link.md §12.3 |
| `0x00618640` | создание текстуры (`CreateTexture`, `+0x5C`); пул 1 (`MANAGED`) у текстур потоков, 0 (`DEFAULT`) у целей отрисовки | live-link.md §12.3; renderer.md §6 |
| `0x00611000` | текстура по хэшу имени: карта `*0x012058E0`, узел — объект текстуры (имя `+0x34`, следующий `+0x38`, ручка `+4`) | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x012058E0` | данные: карта текстур по хэшу имени (4096 корзин по `hash & 0xFFF`) | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00619CEB` | место, где хэшем текстуры берётся последний id ресурсов `tg4h`/`tg4d` | live-link.md §12.3 |
| `0x01223558` | данные: загрузчик `tg4` (указатель); загрузки в пути `+0x20` (записи по 0x14 байт), их число `+0x24` | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00E40F18` | данные: таблица методов загрузчика `tg4` | live-link.md §12.3 |
| `0x00619CB0` | загрузчик `tg4`: загрузка | live-link.md §12.3 |
| `0x00619D70` | загрузчик `tg4`: выгрузка | live-link.md §12.3 |
| `0x00619EF0` | пишет команду «залить уровни» в буфер кадра: `0x011278A4`, ручка, размер, данные | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x011278A4` | данные: команда «залить уровни текстуры» (её исполняет поток рендера) | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x0061A5F0` | поток рендера исполняет команду заливки (→ `0x00619F50` → `0x00606AC0`) | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00619F50` | промежуточный шаг исполнения команды заливки | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00606AC0` | заливка уровней текстуры | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x00606A40` | заливка одного уровня: `LockRect`, копия, `UnlockRect` | live-link.md §12.3 |
| `0x00606FD0`, `0x00618A70` | `LockRect` уровня (vtable `+0x4C`) | live-link.md §12.3 |
| `0x00614ED0` | копия данных уровня с учётом шага строки | live-link.md §12.3 |
| `0x006071A0` | `UnlockRect` | live-link.md §12.3 |
| `0x01127890` | данные: ответная команда после заливки (→ `0x00619EA0`) | live-link.md §12.3 |
| `0x00619EA0` | отмечает загрузку в пути, если она есть | live-link.md §12.3; crates/doctor-toolkit-live/src/addr.rs |
| `0x0110A450` | данные: таблица встроенных текстур `{имя, 0xAARRGGBB}` (`color`, `normal`, `specular`, `occlusion`, `lightmap`), создаются при запуске | renderer.md §10.7 |

## Что неизвестно

Как 23 вызывающих `0x00606C80` переносят отказ (`0xFFFFFFFF`, нет свободной записи) — не разобрано. Текстуру другого размера или формата на месте не подменить.

## Подробнее

- [Живая связь](../../analysis/live-link.md) §12.3 «Текстуры без перезапуска»
- [Рендер](../../analysis/renderer.md) §6 «Текстуры: таблица 4096 и пул MANAGED»
- [Типы ресурсов](../../formats/resource-types.md) §1 (TG4)
