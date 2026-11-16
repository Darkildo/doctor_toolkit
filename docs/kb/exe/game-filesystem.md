---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 32, 46]
sources: ["docs/analysis/launch-flags.md §2", "docs/analysis/live-link.md §14.1", "docs/formats/audio.md §9", "godfather2.exe 0x0068BD30"]
tags: [exe, files, archives]
updated: 2026-11-16
---
# Файловая система игры, архивы и cmd.txt

Адреса — виртуальные, издание PC 1.0.789.0. Все файлы данных игра открывает через свою файловую систему (путь в UTF-8 под корнем игры), архивы `GODFATH0…12.VIV` монтируются при запуске. Командную строку игра не читает — только `cmd.txt`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x01223378` | данные: объект файловой системы игры | launch-flags.md §2 |
| `0x004015E0` | файловая система: открыть | launch-flags.md §2 |
| `0x00401970` | файловая система: размер | launch-flags.md §2 |
| `0x00401780` | файловая система: прочитать | launch-flags.md §2 |
| `0x004019A0` | проверка существования файла пробным открытием | live-link.md §14.1 |
| `0x00402E90` | построение пути `cmd.txt` | launch-flags.md §2 |
| `0x00AC0AD0` | открытие файловой системы игры: относительный путь получает корень, UTF-8 → UTF-16, `CreateFileW` | live-link.md §14.1 |
| `0x00ABD6E0` | открытие класса файла (через `CreateFileA`) | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ABD7D0` | чтение класса файла (слот `+0x1C` таблицы `0x00D9AFC0`) | live-link.md §16.5 |
| `0x00D9AFC0` | данные: таблица методов класса файла | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00ACF580` | ещё одно открытие через `CreateFileW` (бит `0x200` → `CREATE_ALWAYS`); на стенде не вызывалось | live-link.md §14.1 |
| `0x00ACEFC0` | чтение `ReadFile` синхронными кусками по 128 КБ, без `OVERLAPPED` | live-link.md §16.5; performance.md §4 |
| `0x00ABEC10` | чтение через `ReadFile` (по дескриптору потоков) | live-link.md §16.5; performance.md §4 |
| `0x00C724C0` | обход папки (`путь/*`, `GetFileAttributesA`, `FindFirstFileA`) | live-link.md §14.1 |
| `0x0068BD30` | функция запуска движка (один вызов — из `0x0068D010`); в ней цикл монтирования архивов `GODFATH0.VIV` … `GODFATH12.VIV` | audio.md §9; live-link.md §14.1 |
| `0x0068BF87` | место в `0x0068BD30`: начало цикла монтирования — `%sGODFATH%d.VIV` для 0…12 (число 13 — `0x00E2F000`), проба `0x004019A0`, для найденного архива — `0x004014F0` с его путём | live-link.md §14.1 |
| `0x00E2F000` | данные: число архивов, 13 | live-link.md §14.1; audio.md §9 |
| `0x00B901D0` | `rw::core::BigHandler::AddArchiveFromFile`: с флагом 0 добавляет архив в конец списка | audio.md §9 |
| `0x00B90FB0` | открытие файла из архивов: идёт по списку с начала и берёт первый архив, где имя есть | audio.md §9 |
| `0x006987C0` | запись файла профиля | live-link.md §14.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00699C40` | чтение файла профиля (открытие) | crates/doctor-toolkit-live/src/addr.rs |
| `0x00697ED0` | чтение файла профиля (через `ReadFile` в `EDI`) | live-link.md §16.5; crates/doctor-toolkit-live/src/addr.rs |

## Подробнее

- [Флаги запуска](../../analysis/launch-flags.md) §2
- [Живая связь](../../analysis/live-link.md) §14.1
- [Формат звука](../../formats/audio.md) раздел об архивах
