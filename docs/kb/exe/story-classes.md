---
type: exe
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [77]
sources: ["docs/analysis/story.md §2", "docs/analysis/story.md §3", "docs/analysis/story.md §6", "docs/analysis/story.md §7", "research/class_handlers.py"]
tags: [exe, missions, checkpoints, objectives, rewards]
updated: 2026-11-16
---
# Классы сюжета: чекпоинты, цели, награды

Адреса — виртуальные, издание PC 1.0.789.0. Читатели атрибутов и обработчики сообщений классов заданий найдены по таблицам методов (`research/class_handlers.py`); функции менеджеров прочитаны декомпилятором.

| Адрес | Что это | Источник |
|---|---|---|
| `0x008FBA00` | читатель атрибутов `Checkpoint` (номер `+0x74`, глава `+0x70`, набор потоков `+0xCC`, сообщения `+0x10C…+0x124`) | analysis/story.md §3.1 |
| `0x008FC1A0` | обработчик `Checkpoint`: регистрация в менеджере, сообщение `load`, `iMsgStreamSetLoadComplete` | analysis/story.md §3.3 |
| `0x008FD320` | менеджер чекпоинтов: добавить чекпоинт в список загруженных | analysis/story.md §3.3 |
| `0x008FC9E0` | менеджер чекпоинтов: сделать чекпоинт текущим (ключи телеметрии `CHKC`, `CHKS`) | analysis/story.md §3.3 |
| `0x008FBFE0` | `Checkpoint`: запросить набор потоков или сразу послать себе `iMsgStreamSetLoadComplete` | analysis/story.md §3.3 |
| `0x008FC090` | `Checkpoint`: разослать `on_loaded`, затем `on_started` или `on_restarted` | analysis/story.md §3.3 |
| `0x008FCB40` | менеджер чекпоинтов: перезапуск текущего чекпоинта (ключ `CHKR`) | analysis/story.md §3.3 |
| `0x008FCE70` | менеджер чекпоинтов: провал (ключ `CHKF`, `iMsgCheckpointFailed`, эффект `accomp_blood`) | analysis/story.md §3.5 |
| `0x008FD0F0` | менеджер чекпоинтов: запись в сохранение (номер чекпоинта или его атрибут 14, матрица игрока) | analysis/story.md §5 |
| `0x008FD510` | менеджер чекпоинтов: чтение из сохранения, запуск чекпоинта с сохранённым номером | analysis/story.md §5 |
| `0x008FC310` | читатель атрибутов `CheckpointFailure` | analysis/story.md §2 |
| `0x008FC5D0`, `0x008FC470` | обработчик `CheckpointFailure` (`0x008FC5D0` → `0x008FC470`): провал, если чекпоинт атрибута 2 текущий или не задан | analysis/story.md §3.5 |
| `0x0094E8A0` | читатель атрибутов `UIHudObjective` | analysis/story.md §6.1 |
| `0x0094E9D0` | обработчик `UIHudObjective`: `show` → `0x0094EE30`, `hide` → `0x0094F0F0` | analysis/story.md §6.1 |
| `0x011307AC` | данные: менеджер целей (указатель); очередь на 8 целей | analysis/story.md §6.1 |
| `0x0094EE30` | менеджер целей: поставить цель в очередь показа | analysis/story.md §6.1 |
| `0x0094EEA0` | менеджер целей: показать очередную цель и запомнить её текст как текущую цель | analysis/story.md §6.1 |
| `0x0094F0F0` | менеджер целей: убрать цель с экрана или из очереди | analysis/story.md §6.1 |
| `0x00939AF0` | интерфейс: вызов Flash `ShowObjective` (заголовок, текст), рассылка `iMsgHudObjectiveShown` | analysis/story.md §6.1 |
| `0x00939B50` | интерфейс: вызов Flash `HideObjective` | analysis/story.md §6.1 |
| `0x00915F90` | вызов Flash `ShowCurrentObjective` с хэшем текущей цели `[*0x01129938+0x28]` (единственное место со строкой; на стенде цель видна во «Взгляде дона») | analysis/story.md §6.2 |
| `0x009501D0` | читатель атрибутов `UIJobInfo` | analysis/story.md §6.3 |
| `0x00950620` | обработчик `UIJobInfo`: показать и убрать услугу в списке «Взгляда дона» | analysis/story.md §6.3 |
| `0x00950BE0` | `UIJobInfo`: метка на карте «Взгляда дона» (`dv_job_giver`, `dv_mission_task`) | analysis/story.md §6.3 |
| `0x0093F130` | читатель атрибутов `UIFanfare` | analysis/story.md §2 |
| `0x0093F240`, `0x0093F510` | обработчик `UIFanfare` (`0x0093F240` → `0x0093F510`): надпись через экран | analysis/story.md §7 |
| `0x00891530` | читатель атрибутов `JobReward` | analysis/story.md §7 |
| `0x00891AE0`, `0x00891A50` | обработчик `JobReward` (`0x00891AE0` → `0x00891A50`): выдать награду услуги | analysis/story.md §7 |
| `0x008910D0` | читатель атрибутов `ExecuteScoreEvent` | analysis/story.md §2 |
| `0x008911E0` | обработчик `ExecuteScoreEvent`: исполнить событие базы очков (`0x00894C90`) | analysis/story.md §7 |
| `0x00894C90` | база очков: исполнить событие по хэшу | analysis/story.md §7 |
| `0x008F0F40` | читатель атрибутов `AutoSave` | analysis/story.md §2 |
| `0x008F1230`, `0x008F1160` | обработчик `AutoSave` (`0x008F1230` → `0x008F1160`): автосохранение по сообщению | analysis/story.md §5 |
| `0x008FD7C0` | читатель атрибутов `HitMission` | analysis/story.md §2 |
| `0x008FEC30` | обработчик `HitMission` (не разобран) | analysis/story.md §7 |
| `0x008E1FF0` | читатель атрибутов `DynamicJob` | analysis/story.md §2 |
| `0x008E5140` | обработчик `DynamicJob` (не разобран) | analysis/story.md §2 |
| `0x008E6CD0` | читатель атрибутов `DynamicJobInvoker` | analysis/story.md §2 |
| `0x008E6E70` | обработчик `DynamicJobInvoker` (не разобран) | analysis/story.md §2 |
| `0x00895FA0` | читатель атрибутов `TestScore` | analysis/story.md §2 |
| `0x008961D0` | обработчик `TestScore` (не разобран) | analysis/story.md §2 |
| `0x009065C0` | читатель атрибутов `UnlockCheck` | analysis/story.md §2 |
| `0x00906860` | обработчик `UnlockCheck` (не разобран) | analysis/story.md §2 |
| `0x0094F7A0` | читатель атрибутов `UIHudTutorial` | analysis/story.md §2 |
| `0x0094F9E0` | обработчик `UIHudTutorial` (не разобран) | analysis/story.md §2 |

## Что неизвестно

Поведение `HitMission`, `DynamicJob`, `DynamicJobInvoker`, `TestScore`, `UnlockCheck`, `UIHudTutorial`; перезапуск после провала (`0x008FD6B0`); формат базы очков — см. [вопрос](../questions/story-open-items.md).

## Подробнее

- [Сюжет и миссии: разбор](../../analysis/story.md) §2–§7
- [Чекпоинты и переменные: адреса](missions-variables.md), [обработчики логических классов](logic-class-handlers.md)
- [Сюжет и миссии](../systems/story.md)
