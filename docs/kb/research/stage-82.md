---
type: research
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [82]
sources: ["docs/analysis/crew.md", "docs/analysis/game-bench.md §44", "examples/crew/new-crew-member.sh", "research/crew_survey.py", "research/bench_crew.sh", "crates/doctor-toolkit/tests/corpus/crew.rs", "data/gf2/class_behavior.toml"]
tags: [research, crew, family, made-men]
updated: 2026-11-16
---
# Этап 82: Команда

- Состояние: сделан (2026-11-16).
- Зависит от: [этап 77](stage-77.md), [этап 79](stage-79.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- Семья и команда игрока: места древа, уровни и сообщения, которые их открывают, три пути в семью, повышение, приговор и гибель, оружейная лицензия и заслуги — [команда игрока](../systems/crew.md), [разбор](../../analysis/crew.md), [адреса](../exe/player-crew.md).
- Записи `NPC_CRW_*`: 81 шаблон рекрутов в `gf2/globals_global`, их `SimNpc`, специальности, внешность, тексты; где игра их расставляет — `research/crew_survey.py`, корпусный тест `crew.rs`.
- Классы прочитаны в коде: [AddPlayerFamilyMember](../classes/AddPlayerFamilyMember.md), [UnlockCheck](../classes/UnlockCheck.md); атрибуты [NPCCrewComponent](../classes/NPCCrewComponent.md) и [SimNpc](../classes/SimNpc.md). Новые списки значений: [crew_rank](../enums/crew_rank.md), [weapon_licence](../enums/weapon_licence.md).
- Сохранение: число в записи члена семьи — оружейная лицензия; разобрана запись члена семьи в блоке `families` (место, ранг, специальности, оружие) — [сохранения](../systems/saves.md); команда `save` это показывает.
- Персонажи дополнений: лежат в данных игры, закрыты `UnlockCheck` по флагу в объекте денег игрока; как их открывает сторонний патч.
- Пример `examples/crew/new-crew-member.sh` и его проверка в игре — [опыт](../experiments/new-crew-member.md).

## Что осталось

- [Портрет нового человека](../questions/crew-portrait-of-new-member.md) — голова по умолчанию, причина не найдена.
- [Капо `crosato_0051`](../questions/capo-crosato-0051-missing.md) — стало догадкой: это человек чести семьи Кармине с рангом 4.
- Специальность в деле, ранение и гибель на стенде, рекруты дополнений на стенде — не проверялись.

## Правила этапа

Этап обновил базу знаний: см. [как обновлять базу](../README.md) и [карту исследований](../maps/research.md).
