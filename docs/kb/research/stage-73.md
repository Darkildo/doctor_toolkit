---
type: research
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [73]
sources: ["docs/logic-graph.md §2.1", "data/gf2/game_messages.tsv", "data/gf2/enums.toml", "data/gf2/attr_types.toml", "data/gf2/messages.toml", "research/message_catalog.py", "research/enum_values.py", "research/attr_types.py"]
tags: [research, types, messages]
updated: 2026-11-16
---
# Этап 73: Типизация сообщений и атрибутов

- Состояние: сделан (2026-11-16) по данным, коду и корпусу; стенд не запускался.
- Зависит от: [этап 72](stage-72.md).
- Целевое издание: только PC 1.0.789.0.

## Что сделано

- **Сообщения — открытое перечисление** `MessageId`: `Engine` (571 имя из exe), `Game` (41 674 имени из SimGroup игры), `Custom` (имя мода с проверенным хэшем), `Unknown(хэш)`. Каталог `data/gf2/game_messages.tsv` — 41 872 сообщения: кто шлёт и кто слушает (класс, атрибут, число объектов), область, кто ещё называет имя. См. [сообщения файлов игры](../messages/game-messages.md) и [сообщения движка](../messages/engine-messages.md).
- **Смысл атрибутов**: 210 `MsgIn` и 143 `MsgOut` из схемы пинов и ещё 468 атрибутов в `data/gf2/attr_types.toml` — 267 `Ref<Класс>`, 115 `Res<тип>`, 41 `Enum<список>`, 35 `Seconds`, 4 `Money`, 6 `Bool`; уверенность `data` — 306, `exe` — 29, `guess` — 133. Колонка «Смысл» появилась в заметках [классов](../classes/index.md).
- **Перечисления** (`data/gf2/enums.toml`, заметки [перечислений](../enums/index.md)): семьи 28, города 9, заведения 122, типы зданий 9 (без имён), оружие и предметы 36, специализации команды 6 (из exe), категории звука 49, эмоции 7.
- **Один источник**: из `data/gf2/` `build.rs` делает типы Rust (`doctor_toolkit::game_types`), `doctor-toolkit types-gen` — модуль Python с заглушкой и JSON Schema; те же данные — в выпадающих списках редактора логики и редактора скриптов, в MCP (`game_types`) и в этих заметках.
- **Проверка правок**: новое имя сообщения с хэшем сообщения игры — ошибка; «слышат, но никто не посылает» и наоборот — предупреждения (`doctor-toolkit logic-check`, MCP `validate`, сборка мода; новые сообщения мода — поле `messages` в `mod.json`).
- Корпусные тесты сверяют каталог, списки и все 468 типов с файлами игры.

## Что выяснилось по дороге

- В файлах игры нет двух имён сообщений с одним хэшем: 41 872 имени — 41 872 хэша.
- `Building` 3 — хэш города, `Building` 4 — хэш семьи-владельца; всего хэш семьи лежит в 15 атрибутах (`Sentient` 2, `GenomeFilter` 3 и 4, `TestBuildingOwnership` 1, `ChangeBuildingOwnership` 2 …), хэш города — в 8, номер заведения — в 17.
- Ресурс `pad` — 7 эмоций набора `pedestrian`, в записи лежит `sdbm(lower(имя))`; ресурс `auc` — 49 категорий звука с родительской группой (`DIALOG`, `MUSIC`, `SFX`).
- Числа корпуса в документах устарели после правок схемы пинов: было 41 871 сообщение, 58 013 отправок, 72 302 подписки; сейчас 41 872, 57 881 и 72 633. «574 сообщения движка» — на самом деле 571 имя.
- Объектов `Building` 128 (113 в одиночной игре и 15 на картах мультиплеера), номеров заведений — 122.

## Не сделано и открыто

- Данные сообщений (`data`) — `unknown` у всех: [вопрос](../questions/lua-message-data.md).
- Кто в коде движка шлёт сообщения `Engine`: [вопрос](../questions/engine-message-senders.md).
- Отправители вне SimGroup известны только как «имя стоит в ресурсе»: [вопрос](../questions/non-simgroup-message-senders.md).
- Имена типов зданий: [вопрос](../questions/building-type-names.md).
- `Bool` и `Money` — догадки: [вопрос](../questions/attr-bool-and-money-types.md); точное деление видов значений — [вопрос](../questions/simgroup-value-kinds.md).
- Ссылки на звук без типа: [вопрос](../questions/audio-reference-ids.md).
- Отдельной заметки на каждое сообщение игры нет; полной JSON Schema всего `mod.json` нет (только поле `messages`); выпадающие списки в окне редактора не проверялись снимком экрана.

## Подробнее

- [Граф логики](../../logic-graph.md) §2.1
- [Карта логики](../maps/logic.md), [карта исследований](../maps/research.md)
