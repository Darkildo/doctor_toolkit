---
type: system
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 23, 45]
sources: ["docs/analysis/tunables.md §1–§3", "data/gf2/tunables.toml", "docs/analysis/game-bench.md §31"]
tags: [parameters, tunables, wizard, simgroup, balance]
updated: 2026-11-16
---
# Параметры игры (мастер «Изменить параметры игры»)

Список `data/gf2/tunables.toml` — 75 параметров, которые мастер даёт менять без знания форматов. Все они — атрибуты объектов SimGroup ресурса `gf2/globals_global:simgroup:00000000` (поток загружается всегда).

## Как устроен список

- Имена атрибутов SimGroup в исполняемом файле не хранятся, поэтому параметр задан как «класс пакета + индекс атрибута + объекты» (см. [объекты SimGroup](../engine/simgroup-objects.md)).
- Запись таблицы `tunable`: `id`, `category`, `label`, `description`, `source = "simgroup"`, `resource`, `class`, `attribute`, `objects` (GUID), `object_names`, `kind` (`float`/`int`), `min`, `max`; в начале файла `version = 1`.
- Категории и число записей: `weapons` 27, `vehicles` 12, `player` 11, `crew` 6, `police` 6, `world` 5, `health` 4, `economy` 4.
- Объекты выбираются независимо от описываемого атрибута: оружие — по ссылкам `Player` 113–145, улучшения — по имени в `UpgradeData` 0, семьи — по имени в `Family` 1, NPC и машины — по модели, погони — по цепочке `Family` 7 → `ChaseConfigList` → `ChaseConfig`.
- Смысл атрибута — по закономерности значений между объектами одного класса; где она не однозначна, описание говорит «probably».
- Имя атрибута из metamodel есть только у `StandardDamageComponent` 0 = `CMD_SetInitialHealth` (здоровье игрока 4500, NPC и машин) — самые надёжные записи.
- Список собирает и проверяет `research/tunables.py` (`build`, `check`, `survey`): 75 записей, 0 ошибок, ни один выбранный объект не повторяется в других потоках.
- Запись оружия покрывает все четыре уровня улучшения одним значением (`objects` — четыре `Gun`).

## Что проверено в игре

- `Gun` 6, 7, 8 — урон, магазин и запас (см. [оружие](weapons.md)).
- `UpgradeData` 35 (Heart — секунды жизни после ранения) и 31 (Health Regen) (см. [улучшения](upgrades.md)).
- Остальные записи — по данным: «имя» (здоровье), «сильная» закономерность или «probably».

## Что неизвестно

- Не проверены в игре: `Family` 8 (сила семьи, см. [семьи](families.md)) и `Reduced Law Costs` 19/20.
- В список не вошли: доходы предприятий и рэкета, цены улучшений, взятки (в SimGroup их нет, см. [экономика](economy.md)); деньги с убитых NPC; скорость и масса машин (см. [транспорт](vehicles.md)); настройки ИИ (см. [ИИ](ai.md)) и полиции, кроме порогов розыска (см. [полиция](police.md)).
- Связи «смещение в памяти → индекс атрибута» для полей таблицы Cheat Engine («Damage», «Rate Of Fire», таймеры хода семьи) нет.
- metamodel и `variabledictionary` как источники параметров не используются: первое задаёт здоровье разрушаемых частей конкретных моделей, второе — флаги сюжета.

## Подробнее

- [Параметры игры](../../analysis/tunables.md) §1–§3
- [Стенд проверки в игре](../../analysis/game-bench.md) §31
