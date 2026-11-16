---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 10, 45, 63]
sources: ["docs/analysis/game-bench.md §20.4", "docs/analysis/game-bench.md §30", "docs/analysis/game-bench.md §31.1", "docs/analysis/tunables.md §2"]
tags: [weapons, giveitemrelay, gun, tunables, model-import]
updated: 2026-11-16
---
# Оружие в руке: выдача, параметры Gun и новая модель

**Этап 45 — не получилось.** Копия `GiveItemRelay` задания (`hitstream31`, объект 41, «Томми-ган»)
в SimGroup `gf2/cuba` с сообщением `doctor_give_tommy`: `live send` — «1 listener», но в руках пусто.

**Этап 63 — условие найдено.** `GiveItemRelay` игроку работает, если цель — сам игрок:
`AttributeRelay.0 = 0`, `AttributeRelay.2` = GUID игрока (`04bdc758…`); с `AttributeRelay.0 = 2` (как
`give_pistol` сетевых карт) — нет. `GiveItemRelay.3 = 20` ломает выдачу; повторная выдача патронов не
добавляет.

**Третья волна — подтверждено.** Копия объекта 16 `cp_739_zonestream` в `yuri/medium/zone4` (`level
add` без модели + `simgroup-set`), сообщения `doctor_give_tommy`/`doctor_give_pistol`:

- «Томми-ган» 40-60 и пистолет 8-8 в руке. `GiveItemRelay.0` — сообщение, `.1` — `Gun` (автомат ур. 1
  — объект 104 группы `globals_global`, пистолет ур. 1 — объект 115).
- Счётчик «A-B»: A — в магазине, B — в запасе. `Gun.7` (магазин) 13 и 3 → «13-39» и «3-9». `Gun.8` —
  предел запаса: без правки выдано 100 и 16 (меньше 200 и 32); при 52 и 12 выдано ровно 52 и 12.
- Урон `Gun.6`: мишень — стоящая машина (`AICarBehavior`), игрок в 8 м. Без правки — 100 патронов,
  машина цела; с 20000 — огонь после второго одиночного выстрела, взрыв в следующей очереди.
- Новая модель: меш `thompson` (`globals_global`) заменён `model-import` на ту же модель ×2 по длине,
  ×1,6 в толщину, с красной текстурой — в руке большой красный автомат; значок оружия на экране
  прежний (отдельная текстура).

Выбор оружия на стенде — колесо оружия: удерживать пробел и сдвинуть мышь; прицел и выстрел —
`xdotool mousedown 3` и `mousedown 1`.

Что не проверено: урон по человеку (захват цели не срабатывает, NPC убегают).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §20.4, §30, §31.1
- [Параметры игры](../../analysis/tunables.md) §2 (оружие)
- [Устройство Нью-Йорка](../../analysis/queens.md) §8
