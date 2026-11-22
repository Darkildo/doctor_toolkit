---
type: exe
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [114]
sources: ["docs/analysis/dev-menu-cheat-table.md §0.1", "docs/analysis/dev-menu-cheat-table.md §4.2", "docs/analysis/dev-menu-cheat-table.md §4.4", "docs/analysis/dev-menu-cheat-table.md §4.5", "docs/analysis/dev-menu-cheat-table.md §4.6", "docs/analysis/dev-menu-cheat-table.md §4.7"]
tags: [exe, family, war, venues]
updated: 2026-11-20
---
# Семьи: люди, вендетта, заведения, услуги

Адреса — виртуальные, издание PC 1.0.789.0; проверено в Ghidra на этапе 114. У семьи массив записей людей `Family+0x1A4` (число `+0x1A8`); запись: `+4` состояние (1, 2 — жив; 3, 4 — при заведении, с таймером; 5 — больница; 6 — тюрьма; 7 — мёртв), `+8` `SimNpc`, `+0xC` семья, `+0x14` таймер, `+0x18` пост (id заведения).

| Адрес | Что это | Источник |
|---|---|---|
| `0x008BF280` | `Family::SetMemberState(SimNpc, состояние, пост, f32 таймер)`, `thiscall`, `RET 0x10`; мёртвых не трогает | dev-menu-cheat-table.md §4.4 |
| `0x008BF340` | ранить: снять с поста, 5, таймер `Family+0x1D8 × запись+0x20` | dev-menu-cheat-table.md §4.4 |
| `0x008BD890` | в тюрьму `(запись)`, `RET 4`: 6, срок `Family+0x1DC × запись+0x24`; по `SimNpc` — `0x008BF3D0` | dev-menu-cheat-table.md §4.4 |
| `0x008BD8E0`, `0x008BD920` | выпустить (6 → 1), вылечить (5 → 1) `(запись)` | dev-menu-cheat-table.md §4.4 |
| `0x008C1350` | `Family::KillMember(SimNpc)`, `thiscall`, `RET 4`: снять с поста, 7 | dev-menu-cheat-table.md §4.4 |
| `0x008C8750` | человек выведен из строя: смерть `0x008C1350` и событие `0x007E8FF0(семья, 0x6A194E56)`, тюрьма или больница | dev-menu-cheat-table.md §4.4 |
| `0x008C8A20` | запись: `SetState(состояние, f32)`, `RET 8`; оповещение `0x008C83B0` | dev-menu-cheat-table.md §4.4 |
| `0x008C8A80`, `0x008C80D0` | запись: снять с поста и задать состояние; задать пост | dev-menu-cheat-table.md §4.4 |
| `0x008BF420` | последний активный человек семьи (состояние 1…4, ранг < 6) — не «убить» | dev-menu-cheat-table.md §4.4 |
| `0x008C0726…0x008C07A5` | читатель атрибутов `Family` 18–22 → `+0x1D8…+0x1E8`: больница, тюрьма, таймеры состояния 3 (`+0x1E0`, `+0x1E4`) и 4 (`+0x1E8`, `0x008C1120`) | dev-menu-cheat-table.md §4.7 |
| `0x008BE7C0`, `0x008BE920`, `0x008BE890` | вендетта `Family+0x18C` `{хэш, f32}` (число `+0x190`, максимум `+0x198`): индекс по хэшу, запись по индексу, пересчёт максимума | dev-menu-cheat-table.md §4.5 |
| `0x006B8500` | действие `ChangeBuildingOwnership` (`fastcall`): `+0x58` id, `+0x5C` семья, `+0x60` флаги (бит 0 — без оповещения), `+0x64` | dev-menu-cheat-table.md §4.2 |
| `0x006B0EE0` | заведение по id: менеджер `*0x0112A558` `+0x28`/`+0x2C` (запись = объект `+0x180`), затем `+0x34` | dev-menu-cheat-table.md §0.1 |
| `0x006B68C0` | `Venue::SetOwner(хэш, 0, 0, 0)`, `thiscall`, `RET 0x10` | dev-menu-cheat-table.md §4.2 |
| `0x006BE8A0` | оповещение о захвате `(новая семья, старая семья, venue)`, `cdecl` | dev-menu-cheat-table.md §4.2 |
| `0x011299C0` | данные: менеджер услуг `JobReward` (`+4` список, `+8` число) | dev-menu-cheat-table.md §4.6 |
| `0x00891A50` | `JobReward::Give`: +1 к `+0x174` или деньги (бит 0 `+0x170`), сообщение `+0x5C` | dev-menu-cheat-table.md §4.6 |

Не выяснено: смысл таймеров хода `Family+0x144…+0x158` и сколько захватов за кадр игра переносит (этап 123, стенд).

## Подробнее

- [Таблица Cheat Engine → мод](../../analysis/dev-menu-cheat-table.md) §4
- [Семьи и деньги](family-money.md)
- [Команда игрока](player-crew.md)
