---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 47]
sources: ["docs/analysis/live-link.md §15.1", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, collision, havok, world]
updated: 2026-11-16
---
# Части коллизии Havok у объектов мира

Адреса — виртуальные, издание PC 1.0.789.0. Часть `CollisionHavok4Part` держит тела Havok объекта. Модуль правки мира после сдвига вынимает тела из мира и кладёт обратно по новой матрице; часть коллизии он узнаёт по слоту `+8` её таблицы методов.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00E358B8` | данные: таблица методов `CollisionHavok4Part` | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00D7C188`, `0x00D7C240`, `0x00D7C488` | данные: таблицы методов частей коллизии разрушаемых объектов | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x004B6810` | слот `+8` всех этих таблиц («есть тело»): метка части коллизии для модуля | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x004B6330` | кладёт тела в мир по текущей матрице объекта (ничего, если бит 0 `+0x28` уже стоит) | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x004B59D0` | перевод матрицы объекта в `hkTransform` (из `0x004B6330`) | live-link.md §15.1 |
| `0x004B66A0` | вынимает тела из мира (`thiscall` без аргументов; только при бите 0 `+0x28`) | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x008CAC10`, `0x008CAC30`, `0x008CAAF0`, `0x008CAB10` | обёртки разрушаемых объектов, зовут `0x004B6330` и `0x004B66A0` | live-link.md §15.1 |
| `0x004B7010` | обновление части по сдвигу (`+0x30`), зовёт `0x004B6C20` | live-link.md §15.1 |
| `0x004B6C20` | перенос «от тела к объекту»; пропускает фиксированные и кинематические тела | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §15.1 «Матрица объекта и что за ней следует»
