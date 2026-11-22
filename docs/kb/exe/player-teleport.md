---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 34, 24, 114]
sources: ["docs/analysis/live-link.md §3", "docs/analysis/live-link.md §5", "docs/analysis/live-link.md §2", "docs/analysis/game-bench.md §13.2", "docs/analysis/game-bench.md §18", "docs/analysis/dev-menu-cheat-table.md §1.5", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, player, teleport]
updated: 2026-11-20
---
# Игрок: список, матрица, телепорт

Адреса — виртуальные, издание PC 1.0.789.0. Игрок берётся из списка игроков; его матрица — в `+0xD0`, как у любого `Entity` (локальная матрица узла `+0x90`; без родителя равна мировой, см. [матрицу Entity](entity-matrix.md)). Телепорт модуля повторяет то, что делает класс `PlayerTeleportRelay`, и должен идти внутри кадра.

| Адрес | Что это | Источник |
|---|---|---|
| `0x012233A0` | данные: список игроков (указатель): `+4` массив, `+0xC` число; запись указывает на `0x1F30` байт внутрь `Player` | live-link.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x007DD020` | `PlayerTeleportRelay`: игрок = `список[0] − 0x1F30`, проверка состояния `+0x24A0` | live-link.md §5; crates/doctor-toolkit-live/src/edition.rs |
| `0x007DD044` | место `PlayerTeleportRelay` с `add ebx, 0xFFFFE0D0` (смещение записи в `Player`) | live-link.md §3; crates/doctor-toolkit-live/src/addr.rs |
| `0x007DD050` | место проверки бита 7 слова состояния игрока `+0x24A0` | crates/doctor-toolkit-live/src/addr.rs |
| `0x007DD062` | место вызова виртуального `+0x14C` игрока с `(0x46F5C7B4, 1, 0)` перед телепортом | crates/doctor-toolkit-live/src/addr.rs |
| `0x007DD0FD` | место вызова `Player::Teleport` (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x007DD117` | место вызова `Player::Teleport` (по addr.rs) | crates/doctor-toolkit-live/src/addr.rs |
| `0x007DD130` | обработчик сообщения `PlayerTeleportRelay` | live-link.md §5 |
| `0x0079E3C0` | `Player::Teleport(this, const Matrix*, flags, MessageHandle* done, Partitions*)`, `thiscall`, вызываемый снимает 16 байт | live-link.md §5; crates/doctor-toolkit-live/src/addr.rs |
| `0x0093C210` | `Hud::Hide` (скрытие HUD счётчиком `*0x01129C4C+0x60`): телепорт зовёт её по `0x0079E3D6`; сама она `0x0040C1F0` не зовёт — это не начало затемнения (исправлено на этапе 114) | live-link.md §2; dev-menu-cheat-table.md §1.5 |
| `0x0040C1F0` | затемнение экрана: пишет команду в буфер кадра `0x01206880` | live-link.md §2; crates/doctor-toolkit-live/src/addr.rs |
| `0x0040C270` | место затемнения, пишущее в буфер команд кадра (сверяется при проверке издания) | crates/doctor-toolkit-live/src/edition.rs |
| `0x0040E0A0` | положение слушателя звука: берёт менеджер камер и список игроков | live-link.md §3; crates/doctor-toolkit-live/src/edition.rs |
| `0x00550D30` | точка, которую использует таблица Cheat Engine из каталога игры: берёт положение игрока из мировой матрицы | game-bench.md §13.2 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §3, §5 «Телепорт»
- [Стенд](../../analysis/game-bench.md) §13.2, §18
- [«Взгляд дона» и HUD](don-view.md), [урон и здоровье игрока](player-damage.md)
