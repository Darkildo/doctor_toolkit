---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 46]
sources: ["docs/analysis/live-link.md §14.1", "docs/analysis/live-link.md §14.2", "docs/analysis/game-bench.md §24", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, files, hooks, imports]
updated: 2026-11-16
---
# Перехват открытия файлов: CreateFile и GetFileAttributes

Адреса — виртуальные, издание PC 1.0.789.0. Модуль подменяет файлы без записи в игру, перехватывая не слоты импорта (их возвращает загрузчик ASI), а сами места вызова: `call [слот]` становится `call hook; nop`, `mov edi, [слот]` — `mov edi, hook; nop`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00D55120` | данные: слот импорта `CreateFileA` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00D55254` | данные: слот импорта `CreateFileW` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00D5528C` | данные: слот импорта `GetFileAttributesA` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00D55068` | данные: слот импорта `MultiByteToWideChar` | live-link.md §14.1 |
| `0x00D55040` | данные: начало таблицы слотов импорта (в расчёте номера ячейки: `0x00D55120 − 0x00D55040 = 0xE0`) | live-link.md §14.2 |
| `0x00E4E5D8` | данные: начало таблицы поиска импорта (в том же расчёте) | live-link.md §14.2 |
| `0x00E4E6B8` | данные: ячейка таблицы поиска импорта слота `CreateFileA` | live-link.md §14.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4EF6C` | данные: имя импорта `CreateFileA` | live-link.md §14.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4E7EC` | данные: ячейка таблицы поиска импорта слота `CreateFileW` | live-link.md §14.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4EABC` | данные: имя импорта `CreateFileW` | live-link.md §14.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4E824` | данные: ячейка таблицы поиска импорта слота `GetFileAttributesA` | live-link.md §14.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00E4FC64` | данные: имя импорта `GetFileAttributesA` | live-link.md §14.2; crates/doctor-toolkit-live/src/edition.rs |
| `0x00ABD73D` | `call [0x00D55120]`: открытие класса файла `0x00ABD6E0` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00BCC4DD`, `0x00BCFC1E`, `0x00BCFC3D` | `call [0x00D55120]`: C-рантайм | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00BCC18C` | `mov edi, [0x00D55120]`: C-рантайм (`_sopen`, вызов через `edi`) | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00AC0BDF` | `call [0x00D55254]`: открытие файловой системы игры `0x00AC0AD0` — все потоки, TOC, архивы и `cmd.txt` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ACF5C8` | `call [0x00D55254]`: открытие `0x00ACF580` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x006987FA`, `0x006988FD` | `call [0x00D55254]`: запись файла профиля `0x006987C0` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00699C59` | `call [0x00D55254]`: чтение файла профиля | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00BC45A1` | `call [0x00D55254]`: C-рантайм | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00BC424F` | `mov edi, [0x00D55254]`: C-рантайм (`_wsopen`) | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00C72505` | `call [0x00D5528C]`: обход папки `0x00C724C0` | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00B96224`, `0x00B960AA`, `0x00D3B5E2` | переходники `jmp [слот]`; ни разу не вызываются | live-link.md §14.1 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §14.1, §14.2
- [Стенд](../../analysis/game-bench.md) §24
