---
type: map
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72, 73]
sources: []
tags: [map, logic]
updated: 2026-11-16
---
# Карта: логика

Игровая логика The Godfather II — объекты SimGroup, которые шлют друг другу сообщения, и скрипты Lua. Назад: [карта базы](../index.md).

## Основа

- [Объектная модель: SimGroup, классы, GUID](../engine/simgroup-objects.md) — объект состоит из пакетов атрибутов своих классов.
- [Шина сообщений](../engine/message-bus.md) — объект слушает и шлёт сообщения по имени (хэш SDBM).
- [Lua в игре](../engine/lua.md) — скрипты звука и музыки, свои скрипты.
- Подробно: [граф логики](../../logic-graph.md), [где живёт логика миссий](../../formats/mission-logic.md), [скрипты](../../scripts.md), [формат SimGroup](../../formats/simgroup.md).

## Классы

[Список классов](../classes/index.md) — заметка на каждый класс: хэш, категория, пакеты, атрибуты с ролью (вход, выход, ссылка, ресурс, параметр), именем и уверенностью. Заметки классов генерируются из `data/gf2/`; поведение дописывается руками в разделе «Заметки» каждой.

Часто встречаются в логике миссий:

- рассылка и реле: [MultiManager](../classes/MultiManager.md), [MessageRelay](../classes/MessageRelay.md), [ForwardSender](../classes/ForwardSender.md);
- триггеры: [TriggerBase](../classes/TriggerBase.md), [TriggerAuto](../classes/TriggerAuto.md), [TriggerBox](../classes/TriggerBox.md);
- условия: [AndEventGate](../classes/AndEventGate.md), [OrEventGate](../classes/OrEventGate.md), [VariableWatcher](../classes/VariableWatcher.md), [VariableOperator](../classes/VariableOperator.md);
- общее для объектов: [CSystemCommands](../classes/CSystemCommands.md), [Base](../classes/Base.md).

Код обработчиков — в заметке [обработчики логических классов](../exe/logic-class-handlers.md) и [реестр классов](../exe/simgroup-class-registry.md).

## Типы

- [Сообщения файлов игры: каталог и группы](../messages/game-messages.md) — открытое перечисление `Engine` / `Game` / `Custom` / `Unknown`, кто шлёт и кто слушает каждое из 41 872 сообщений, проверка новых имён.
- [Перечисления](../enums/index.md) — [семьи](../enums/family.md), [города](../enums/city.md), [заведения](../enums/venue.md), [типы зданий](../enums/building_type.md), [оружие и предметы](../enums/weapon.md), [специализации команды](../enums/crew_specialty.md), [категории звука](../enums/sound_category.md), [эмоции](../enums/emotion.md).
- Смысл атрибутов (`MsgIn`, `Ref<Класс>`, `Res<тип>`, `Enum<список>`, `Seconds`…) — колонка «Смысл» в заметках классов; подробно — [граф логики](../../logic-graph.md) §2.1, итог — [этап 73](../research/stage-73.md).

## Сообщения

- [Сообщения](../messages/index.md) — [полный список сообщений движка](../messages/engine-messages.md), [каталог сообщений файлов игры](../messages/game-messages.md) и заметки о ключевых: [`iMsgPostSpawnInitialize`](../messages/imsg-post-spawn-initialize.md), [`boundary_fade_out`](../messages/boundary-fade-out.md), [`mus_play_igc_01`](../messages/mus-play-igc-01.md), [`cp_325_start_now`](../messages/cp-325-start-now.md), [сообщения кадра](../messages/frame-messages.md).

## Открытые вопросы

- Сообщения: [данные сообщения](../questions/lua-message-data.md), [отправители в коде движка](../questions/engine-message-senders.md), [отправитель](../questions/message-sender-unknown.md), [отправители вне SimGroup](../questions/non-simgroup-message-senders.md), [отложенный хэш у 12 классов](../questions/deferred-message-hash-classes.md), [`ExecuteVFX`](../questions/executevfx-show-hide-senders.md).
- Классы: [имена атрибутов и классов](../questions/attribute-and-class-names.md), [виды значений](../questions/simgroup-value-kinds.md), [`Bool` и `Money`](../questions/attr-bool-and-money-types.md), [типы зданий](../questions/building-type-names.md), [ссылки на звук](../questions/audio-reference-ids.md), [`MultiManager` 1–2](../questions/multimanager-attrs-1-2.md), [`TriggerBase` 6, 8–10](../questions/triggerbase-attrs-6-8-10.md), [`TriggerAuto` flags](../questions/triggerauto-flags.md), [`CSystemCommands` 2–3](../questions/csystemcommands-attrs-2-3.md).
- Lua: [объекты и группы](../questions/lua-groups-and-objects.md), [таймеры](../questions/lua-timer-function-by-name.md), [`PlayEarsAudioStream`](../questions/lua-play-ears-audio-stream.md), [полиция и машины](../questions/lua-police-and-cars.md).

Все вопросы — в [списке](../questions/index.md).
