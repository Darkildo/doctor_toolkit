---
type: experiment
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 45]
sources: ["docs/analysis/game-bench.md §31.3", "docs/analysis/tunables.md §2", "docs/analysis/tunables.md §3"]
tags: [tunables, upgradedata, skills]
updated: 2026-11-16
---
# Параметры навыков: подписи проверены в меню игры

Третья волна проверок. `live money --add 500000`, затем Tab (Don's View) → Continue → «More Don's
View» → Family Tree → `Down` ×5 до «Your Family», `Return`, `Return` на Upgrade (мышью не попасть —
курсор меню идёт с другим множителем). В списке навыков описание — строка из атрибута 2 `UpgradeData`
с подставленным числом.

| Параметр | Правка | В игре | Итог |
|---|---|---|---|
| Heart (`UpgradeData` 35) | 17 (в игре 10) | «Extend survival by 17 seconds» | подпись исправлена: секунды жизни после ранения (навык Survival), а не проценты здоровья |
| Health Regen (`UpgradeData` 31) | 1,25 без правки | «Recover health 25% faster» | подтверждено |
| `Family` 8 | — | — | не проверено: сила семьи ни на одном экране после `-stream` не видна |
| Reduced Law Costs B | — | — | не проверено: бонус — кольцо, которого со старта не получить; среди бонусов колец игры такого имени нет |

Что из этого следует: подписи `tunables.toml`/`tunables.md` исправлены (`research/tunables.py`); тем же
способом — строка описания с подставленным числом — проверяются и остальные навыки.

Параметры оружия (`Gun` 6, 7, 8) проверены в той же волне — [оружие в руке](weapon-in-hand.md).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §31.3
- [Параметры игры](../../analysis/tunables.md) §2, §3
