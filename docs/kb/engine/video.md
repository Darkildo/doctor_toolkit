---
type: engine
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 31]
sources: ["docs/formats/video.md §1", "docs/formats/video.md §2", "docs/formats/video.md §3", "docs/formats/video.md §6", "docs/formats/video.md §7", "docs/formats/mission-logic.md §5.1", "docs/formats/mission-logic.md §4.1"]
tags: [video, vp6, movie, ea-xa]
updated: 2026-11-16
---
# Видео: ролики `.vp6`

Видеоролики лежат отдельными файлами в `movies/` и не входят в [потоки](stream-loading.md). Играет их RenderWare Movie (`RWMovie`, `MoviePlayer2`). Ролики на движке (Trinity) — другая система, см. [ролики](../systems/cutscenes.md).

## Что известно

- 59 файлов: `common/` (`attract`, `credits`, `M6FMV`, `mainmenu`, `logo`), `en/` (`FMV01–02`, `TRN01–16`, `load`, `deadspace_trailer`), `es/`, `ru/`. 58 — контейнер EA с видео VP6 1280×720 и звуком EA-XA; `common/logo.vp6` — AIFF, в `godfather2.exe` по имени не упоминается.
- Контейнер EA — поток чанков `[4] тег, u32 размер`: `MVhd` (заголовок: кодек `vp60`, ширина, высота, число кадров, частота 982026 / 32767 ≈ 29,97), `MV0K`/`MV0F` (ключевой и промежуточный кадры), `SCHl`, `SCCl`, `SCDl`, `SCEl` (звук).
- Проигрыватель: конструктор реестров демультиплексоров и декодеров — `0x00D087F0`; опознание контейнера EA — `0x00D0A6C0`, `0x00D0A6F0`; разбор — `0x00D0A9B0`; разбор `SCHl` — `0x00D0DC90` (по умолчанию 1 канал, 48 000 Гц; кодек 10 → `EAXA`, 23 → `EAL3`); второй демультиплексор — AVI (`0x00D0CFB0`).
- Видеодекодер один — `VideoDecoder_Vp6`: заменять ролики нужно на VP6. `MJPG` и `MPEG` встречаются только в таблицах опознания.
- Путь — `movies/%s/%s` (`0x009C7E50`); запуск ролика `0x009C82F0`: третий аргумент 0 — каталог языка озвучки, 1 — языка текста, 2 — `Common`. Главное меню — `mainmenu.vp6` (`0x00936B90`), титры — `credits.vp6` (`0x00925F40`).
- Что где: `en/load.vp6` — заставки EA и Paramount при запуске, `common/attract.vp6` — фон титульного экрана и демонстрационный режим, `common/mainmenu.vp6` — фон главного меню.
- Звук EA-XA (ревизия 3): кадры по 28 сэмплов, коэффициенты те же, что у EA-XAS игрового [звука](audio.md); декодер тулкита совпадает с ffmpeg (`adpcm_ea_r3`) побайтно.
- Субтитры роликов — ресурс `tob` (15): `sdbm` имени ролика, группы `subtitles` и `titles`, записи «хэш строки `lh2`, начало, длительность».
- Среди классов роликов в SimGroup есть `InGameVideoPlayer` (рядом с `TrinityGameSequence` и `ScriptedSequence`).
- Проверено в игре: замена `load.vp6`, `attract.vp6`, `mainmenu.vp6` роликами, закодированными тулкитом, и замена звуковой дорожки.

## Что неизвестно

- Округление в декодере EA-XA самой игры не сверено (разница не больше младшего бита).
- Кодек EALayer3 (23) в роликах не встречается и не поддержан; VP6 с альфой и чересстрочные кадры — тоже.
- Контейнер AVI с VP6 и PCM игра, судя по коду, прочитала бы — не проверено.

## Подробнее

- [Видео: `movies/**/*.vp6`](../../formats/video.md) §1–§7
- [Логика миссий](../../formats/mission-logic.md) §5.1 — `tob`
- [Звук](../../formats/audio.md) §10 — проверка звука роликов
