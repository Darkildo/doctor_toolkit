---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 32]
sources: ["docs/analysis/launch-flags.md §3.2", "crates/doctor-toolkit/src/play/start.rs"]
tags: [exe, launch, spawn, player]
updated: 2026-11-16
---
# Точка появления игрока: глобальный GUID

Адреса — виртуальные, издание PC 1.0.789.0. Флаг `-playerspawn` кладёт GUID точки появления в глобальную переменную; пакет `PlayerSpawn` с совпавшим GUID подписывается на сообщение появления игрока.

| Адрес | Что это | Источник |
|---|---|---|
| `0x004D8F00` | проверка текста GUID | launch-flags.md §3.2 |
| `0x004D9400` | проверка формата: `{`, 36 знаков по шаблону, `}` | launch-flags.md §3.2 |
| `0x00E2DF20` | данные: шаблон текста GUID | launch-flags.md §3.2 |
| `0x004D8F50` | отвергает «пустой» GUID `{BADBADBA-BEEF-BEEF-555A-C1EA11091091}` | launch-flags.md §3.2 |
| `0x00E2DF48` | данные: текст «пустого» GUID | launch-flags.md §3.2 |
| `0x004D8CE0` | перевод текста GUID в 16 байт (→ `0x004D9500`) | launch-flags.md §3.2 |
| `0x004D9500` | разбор GUID: первые три группы — числа little-endian, остальные — байты по порядку | launch-flags.md §3.2 |
| `0x008111A0` | кладёт GUID точки появления в `0x0112DACC` | launch-flags.md §3.2 |
| `0x0112DACC` | данные: глобальный GUID точки появления игрока | launch-flags.md §3.2; launch-flags.md §3 |
| `0x008115A0` | чтение пакета `PlayerSpawn` (класс `0x30AA423A`) | launch-flags.md §3.2 |
| `0x008116F9` | место в нём: точка вида 2 записывает свой GUID, если глобальный ещё пуст | launch-flags.md §3.2 |
| `0x0120E93C` | данные: сообщение появления игрока | launch-flags.md §3.2 |
| `0x004DA550`, `0x004DA7E0` | для новой игры из меню: GUID из текста `XFOWLER102_C8D726A_A11FA3D_v3` | launch-flags.md §3.2 |

## Подробнее

- [Флаги запуска](../../analysis/launch-flags.md) §3.2
