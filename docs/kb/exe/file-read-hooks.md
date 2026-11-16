---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 58]
sources: ["docs/analysis/live-link.md §16.1", "docs/analysis/live-link.md §16.5", "docs/analysis/performance.md §4", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, files, hooks, profile]
updated: 2026-11-16
---
# Перехват чтения файлов: ReadFile

Адреса — виртуальные, издание PC 1.0.789.0. Профиль модуля меряет чтение файлов, перехватывая все места вызова через слот импорта `ReadFile` тем же приёмом, что и открытие файлов. Чтение потоков идёт синхронно кусками по 128 КБ.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00D55268` | данные: слот импорта `ReadFile` (KERNEL32) | live-link.md §16.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00E4E800` | данные: ячейка таблицы поиска импорта слота `ReadFile` | crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4EA76` | данные: имя импорта `ReadFile` | crates/doctor-toolkit-live/src/edition.rs |
| `0x00ACEFE0` | `call [0x00D55268]` в `0x00ACEFC0` | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABD7E8` | `call [0x00D55268]`: чтение класса файла `0x00ABD7D0` | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABEC29` | `call [0x00D55268]` в `0x00ABEC10` (чтение потоков по дескриптору) | live-link.md §16.5; performance.md §4 |
| `0x00BB684D`, `0x00BB6905`, `0x00BB6B31` | `call [0x00D55268]`: библиотека C | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00698947` | `call [0x00D55268]`: файл профиля `0x006987C0` | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00697F12` | `mov edi, [0x00D55268]`: файл профиля `0x00697ED0` (потом `call edi`) | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |
| `0x00B9608C` | переходник `jmp [0x00D55268]`; никто не вызывает | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §16.5 «Перехваты и проверка издания»
- [Производительность](../../analysis/performance.md) §4
