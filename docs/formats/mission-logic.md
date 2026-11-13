# Логика миссий и Lua в The Godfather II (PC)

Где живёт игровая логика, что в ней делает Lua и что моддер правит, чтобы
изменить миссию. Исследование этапа 16: `godfather2.exe` (PE32, база 0x400000)
в Ghidra 12.2, выгрузка ресурсов командами `dump` и `logic-export`.

Обозначения: **[П]** — проверено (адрес в exe, данные игры или запуск в игре);
**[Г]** — гипотеза. Адреса — виртуальные адреса exe.

## 1. Коротко

1. Lua 5.0.2 в игре — **только скриптовый движок звука и музыки**. Логика миссий —
   граф объектов SimGroup, связанных **именами сообщений** (хэш
   `sdbm(lower(имя))`). Lua подключён к той же шине сообщений: скрипт может
   слушать и посылать любые игровые сообщения, читать и писать переменные движка.
2. Чтобы изменить ход миссии, правят **SimGroup** (триггеры, `MultiManager`,
   проверки, спавн, задания) — это инструмент уже умеет (`logic-export`,
   `logic-import`, `simgroup-set`, вкладка логики в GUI). Lua — «клей» для звука
   и новых реакций на сообщения; его правка — этот этап (`docs/formats/lua.md`).
3. Вывод `print` и ошибки скриптов в релизе теряются; ресурс `lua` может быть и
   байт-кодом, и исходным текстом.

## 2. Lua в exe

### 2.1 Ядро и состояния [П]

| Адрес | Функция |
|---|---|
| 0x627840 | `lua_open` (2 вызова: 0x43E5DC, 0x43E79C) |
| 0x625CA0, 0x625D70, 0x626030 | `lua_pushstring`, `lua_pushcclosure`, `lua_settable` |
| 0x629280 | `luaL_loadbuffer` |
| 0x6293C0, 0x6292E0 | `aux_do` (`luaL_dobuffer`) и `callalert` (`_ALERT` или `fprintf(stderr)`) |
| 0x6290C0 | `luaL_loadfile` (`fopen` с диска) |
| 0x636570 | `luaD_pcall`, вызывается как `lua_pcall` без обработчика ошибок |
| 0x628260 | `luaL_openlib` |
| 0x4DAFD0 | хэш движка `h = h*65599 + tolower(c)` (SDBM в нижнем регистре) |
| 0x408A00 | синхронная рассылка сообщения по общей шине (689 вызовов) |

Синглтон `LuaManager` (`*0x012233F4`, конструктор 0x43E140, vtable 0xE3105C)
держит два состояния: `+0x1C` (создаётся при старте приложения, 0x43E5D0) и
`+0x18` (создаётся с объектом симуляции, 0x43E790). Все потребители скриптов
работают с `+0x18`; **[Г]** `+0x1C` в релизе не используется.

Каждая подсистема регистрирует функции объектом-binder с vtable
`{деструктор, Register(L), Unregister(L)}`; регистрация — встроенный
`lua_register`, пара `lua_pushcclosure` + `lua_settable(LUA_GLOBALSINDEX)` или
массив `{имя, функция}` (обход 0x4AEB70).

### 2.2 Библиотеки [П]

Открыты base (с `coroutine`), table, string, math и `tolua` (`type`,
`takeownership`, `releaseownership`, `cast`). Библиотек io, os, debug и loadlib
нет. В base остались `loadfile`, `dofile` и `require`: они читают `.lua` с диска
через `fopen` относительно текущего каталога процесса (`LUA_PATH` = `?;?.lua`).
`print` заменён своей функцией 0x46BCE0 (см. §3.1).

### 2.3 API для скриптов [П]

Функции C в глобальном окружении (адреса реализаций — в сыром отчёте
исследования; здесь по подсистемам):

| Подсистема | Функции |
|---|---|
| Эмиттеры звука | `PlayEmx`, `PlayEntityEmx`, `StopEmx`, `SetEmxVolume`, `SetEmxPitch`, `SetEmxPosition`, `SetEmxAzimuth` |
| Потоки музыки | `PlayStream`, `PrimeStream`, `StartStream`, `StopStream`, `SetStreamMixCategory`, `SetStreamEmitterLocation`, `SetStreamVolume`, `SetStreamChannelVolume`, `GetStreamStatus`, `GetStreamDuration`, `SetStreamPrimeFunction` |
| Эффекты | `SetStreamEffect`, `SetGlobalEffect`, `SetPluginEffect`, `SetAuxEnabled`, `SetStreamChannelEffectSend` |
| Потоки EARS | `PrimeEarsAudioStream`, `PlayEarsAudioStream`, `StopEarsAudioStream`, `SetEarsAudioStreamStopMessage` |
| Огибающие | `SetEnvelopeMap`, `GetEnvelopeValue` |
| Микшер | `SetMixValues`, `ApplyMix`, `RemoveMix`, `SetMasterVolume` |
| Приглушение | `GetDuckCategoryName` |
| Таймеры | `CreateTimer(имяФункции, oneShot, autoDelete)` (функция — глобальная, по имени), `StartTimer(t, мс)` (одноразовый перезапускается тем же вызовом), `SetTimerRate`, `StopTimer`, `DeleteTimer`, `QueryTimer`; время — игровое, `GetSimTime()` в миллисекундах [П, `scripts.md` §1] |
| Скрипты, позиции | `RegisterScriptName`, `GetPositionValues`, `SetPositionValues`, `GetPlayerPosition`, `TEMP_POS`, `CreatePosition`, `GetPlatform` |
| Удары | `GetImpactData` |
| Группы | таблица `GroupManager`: `RegisterGroup`, `FindGroup`, `RemoveFromAllGroups`; у группы `AddMember`, `RemoveMember`, `IsMember`, `Clear`, `GetMemberCount`, `ForEachMember` |
| Переменные движка | `FindVariable(имя)` → объект с `Get`/`Set`, `VariableGet`, `VariableSet` (поиск по `sdbm(lower(имя))`) |
| Время | `GetSimTime` |
| Целые | `UInt32`, `Int32`, `Int32ToNumber` |
| Объекты движка | `Base`, `DeleteBase` |
| Сообщения | `SendMessage(имя [, тип, данные])`, `SendMessageEx`, `MessageReceiverNew` (через Lua-обвязку `NewMessageReceiver(fn)`) |
| Полиция | `GetFootPoliceIntensity`, `GetCarPoliceIntensity` |
| Транспорт | `GetVehicleType`, `GetVehicleSpeed`, `GetVehicleSpeedDelta`, `GetVehicleAcceleration`, `GetVehicleSkidEnergy`, `GetVehicleHealth`, `GetVehicleRainVelocity`, `SetPlayerCarUsingSimpleAudio`, `GetChaseVehicleStats`, `SetChaseVehicleSound` |
| Менеджер | `EclipseLog` (пустая) |

`SetWatchValue` и `ClearWatchValue`, которые вызывает `aud_utils.lua`, в exe
нет: **[Г]** отладочные функции, вырезанные из релиза; их вызов даёт ошибку,
которую поглощает `pcall`.

Кроме того, exe содержит три фрагмента Lua-кода в исходном тексте:
пустые `debug_print(flag, str)` и `debug_screenprint(flag, str)`;
`gFunctionRegistry`, `RegisterFunction(scriptName, functionName, functionAddr)`
и `CallLuaFunction(scriptName, functionName, ...)`; обвязку `MessageReceiver`
(`NewMessageReceiver`, `MessageReceiverReceive`).

## 3. Вывод, ошибки, запуск

### 3.1 `print`, `EclipseLog`, ошибки [П]

- `print` (0x46BCE0) собирает строку как `luaB_print` и передаёт её хуку
  `*(0x0110A6C0)(src, line, name, text)`. Хук по умолчанию 0x46BFC0 — одна
  инструкция `ret`, функцию установки хука никто не вызывает: вывод теряется.
- `EclipseLog` (0x43E5C0) — `mov eax, 1; ret`.
- Ресурсный скрипт выполняется в 0x4CB0C0: `luaL_loadbuffer(L, data, size,
  "LuaScript")`, затем `pcall` без обработчика, коды возврата только сравниваются
  с нулём. Синтаксические ошибки и ошибки выполнения нигде не видны, игра не
  падает. Так же глушатся ошибки `CallLuaFunction` и таймеров.

### 3.2 Кто и когда запускает скрипты [П]

Скрипты запускают объекты SimGroup класса `LuaAudioScript` (хэш `0x07B6C269`):
29 объектов в `gf2/globals_global`; на `aud_havok_vfx.lua` никто не ссылается.

| Атрибут | Тип | Смысл |
|---|---|---|
| 0 | int | флаги; бит 0 — запустить при загрузке |
| 1 | string | стартовое сообщение |
| 2 | string | стоповое сообщение |
| 3 | guid | ссылка на ресурс `lua` |

По стартовому сообщению объект ставит `gGUID`, выполняет весь чанк (верхний
уровень скрипта вызывает `RegisterFunction("<имя>.lua", "startup", f)`),
снимает `gGUID` и вызывает `CallLuaFunction(<имя>, "startup")`. Стоповое
сообщение и деструктор вызывают `"shutdown"`.

Стартовые сообщения посылают другие объекты SimGroup: `TriggerAuto
"audio_init"` в `globals_global` срабатывает сразу после загрузки (так
стартуют `aud_utils.lua`, `aud_pc_car_sets.lua` и другие), музыкальные скрипты
запускает `MultiManager` уровней (`aud_start_mission_1_script`,
`mus_start_master` …).

### 3.3 Исходный текст вместо байт-кода [П по коду]

В `luaL_loadbuffer` передаётся весь ресурс, а Lua 5.0 сама выбирает разбор по
первому байту: `ESC` — байт-код, иначе текст. Парсер в exe есть (им грузится
встроенный код). Значит, ресурс `lua` может содержать и исходный текст.
Инструмент всё равно записывает байт-код: так ресурс совпадает по формату с
оригиналами, а синтаксические ошибки находятся до игры, а не теряются в `pcall`.

### 3.4 Наблюдаемый сигнал при проверке в игре

Вывод и ошибки скриптов не видны, звук на стенде выключен. Сигнал без патча
exe — обращение к файлу: `loadfile("doctor_probe_<метка>")` открывает файл
через `fopen`, и под Wine это видно в `WINEDEBUG=+file` или в `strace`:
несуществующий файл Wine сначала проверяет через `statx`, поэтому нужен
`TRACE_SYSCALLS=openat,newfstatat,statx,…`, одного `openat` мало. **[П]**
этап 28: пробы новых скриптов видны (`docs/scripts.md` §1).

### 3.5 Что выяснено о `LuaAudioScript` [П, этап 28]

- Атрибут 3 — GUID ресурса: байты четырёх ids в little-endian. Ресурс
  ищется по GUID (`0x004211E0`) в момент запуска; не найден — скрипт молча
  не выполняется.
- С битом 0 атрибута 0 скрипт выполняется прямо при чтении атрибутов
  объекта (`0x00480D01` → `0x00480D90`), то есть пока SimGroup ещё
  загружается: объекты после него не созданы, сообщения им теряются.
- В главном меню (`gf2/frontend`) состояния Lua нет (`[LuaManager+0x18]`
  создаётся с симуляцией, `0x0043E790` из `0x00680130`): найденный
  скрипт там роняет игру (`0x00480E03`). Объекты с переменными
  (`VariableSwitch`, `VariableOperator`) в меню тоже роняют игру
  (`0x004420F3`: нет менеджера переменных `*0x01223428`).
- `FindVariable(имя)` ищет переменную по имени, затем по `sdbm`, а если её
  нет — создаёт; созданная — целое Int32 (`2.5` → 2, `true` — ошибка).
- Получатель сообщений вызывается как `f(self, имя, nil, данные)`; имя —
  строка, `SendMessage` синхронный: `VariableOperator`, сработавший на
  сообщение скрипта, уже изменил переменную, когда `SendMessage` вернулся.

## 4. Где живёт логика миссий

### 4.1 Модель [П по данным и коду, частично Г]

Миссия — набор потоков с SimGroup: `gf2/<город>/global/objectives/<глава-миссия>/cp_<NNN>/...`
(например `cuba/global/objectives/chapter60-mission08-killingcastro/cp_737`) и
`gf2/globalfolder/checkpoints/...`. Единица логики — чекпоинт (`cp_NNN`,
классы `Checkpoint`, `TestCurrentCheckpoint`, `CheckpointFailure`).

Объекты связаны не ссылками, а **именами сообщений** в строковых атрибутах
(как ввод-вывод сущностей в движке Source):

- **триггеры** посылают сообщения: `TriggerAuto` (атрибут 0 — имя, 1 — задержка),
  `TriggerBox`, `TriggerHurt` (пакет `TriggerBase`: слушает 0 fire,
  1 enable, 2 disable; посылает 4 при входе с задержкой 5 и 7 при выходе),
  `EnterExitTrigger` (посылает 0 вход, 1 выход, 2 внутри; слушает 4 enable,
  5 disable), `TriggerRandom` (слушает 0; тройки вес/сообщение/задержка
  1/2/3 … до 8), `TriggerDistance`;
- **`MultiManager`** слушает сообщение из атрибута 0 и рассылает пары
  «сообщение, задержка» из атрибутов 3/4, 5/6 … 17/18 (до 8; читатель
  0x47A320). Пример из `cuba/local`:
  по `mus_play_igc_01` уходят `mus_stop_car_radio_content`,
  `aud_start_mission_1_script` (0,2 с), `mus_start_underscore` (0,3 с);
- **логика**: `OrEventGate`, `AndEventGate`, `VariableOperator`,
  `VariableSwitch`, `VariableWatcher`, `TestTimeOfDay`,
  `TestBuildingOwnership`, `TestScore`, `MissionTimer`, `MessageRelay`,
  `ForwardSender`. `VariableSwitch` (читатель 0x476FC0, 29 атрибутов
  0–28) слушает 0, сравнивает переменную 1 со значениями троек
  «значение, сообщение, задержка» 5/6/7 … 26/27/28 (до 8) и при несовпадении
  посылает сообщение по умолчанию 3 с задержкой 4: в игре атрибут 3 —
  `do_nothing` у 31 из 99 объектов, атрибут 5 — 1 у 82, атрибут 8 — 2 у 94;
- **объекты мира**: `InterestingObject` слушает 1 — выключить, 2 —
  включить (обработчик 0x7279B0: 1 снимает объект с учёта, 2 возвращает;
  в данных пары `band_left`/`band_start`), посылает 3 и 4 в начале и конце
  использования; `NodeController` и `EnableDisableStreams` слушают оба
  своих сообщения 0 и 1 (подписка 0x4089B0);
- **действия**: `FuncSpawn`, `SpawnPoint`, `GiveItemRelay`, `ChangeNPCSettings`,
  `SetInvulnerability`, `PlayerTeleportRelay`, `FadeScreenFx`, `SetTimeOfDay`,
  `PhoneMessage`, `UIHudObjective`, `UIFanfare`, `UIHudTutorial`,
  `ExecuteScoreEvent`, `ChangeDialog`, `PlayAudioStream`, `SetAudioMix`,
  `LuaAudioScript`;
- **задания**: `HitMission` (37 объектов), `DynamicJob` (57),
  `DynamicJobInvoker`, `JobReward`, `UIJobInfo`;
- **ролики**: `TrinityGameSequence` (ссылается на `trinity_seq_master`),
  `ScriptedSequence`, `InGameVideoPlayer`.

Всего в корпусе 253 класса объектов SimGroup. Хэш имени сообщения
`sdbm(lower)` и одна шина **[П]**: читатели атрибутов 124 из 136
классов-пакетов с сообщениями пропускают строку через 0x4DAFD0 и кладут хэш
в глобальную таблицу сообщений 0x408260, которую рассылает 0x408A00;
слушатели подписываются (0x4089B0, 0x407E60, 0x473EC0). Остальные 12 держат
строку и хэшируют её позже (направление по данным). Входы, выходы, ссылки и
имена атрибутов всех классов — `data/gf2/logic_schema.toml`, граф и его
правка — `docs/logic-graph.md`.

### 4.2 Что править и что умеет инструмент

| Что | Тип ресурса | Инструмент |
|---|---|---|
| Ход миссии: сообщения, задержки, условия, спавн, награды | `simgroup` | да: `logic-export`/`logic-import`, `simgroup-set`, GUI |
| Переменные | `variabledictionary` | да |
| Сущности (свойства объектов) | `metamodel` | да |
| Звук, музыка, реакции на сообщения | `lua` | да: `lua-decompile`, `lua-compile`, `lua-import`, GUI (этот этап) |
| Граф логики: соединить, разъединить, добавить и удалить узлы | `simgroup` | да: вкладка Logic, `logic-graph`, `messages`, `logic-connect` (`docs/logic-graph.md`) |
| Новые узлы логики | `lua` + объект `LuaAudioScript` | да: визуальные скрипты (`docs/scripts.md`) — граф → Lua, `script-deploy`, редактор в GUI; **[П]** в игре: запуск при загрузке и по сообщению, приём сообщений, `SendMessage` до объектов SimGroup, переменные игры в обе стороны, таймеры |
| Ролики | `trinity_seq_master` | да: шкала времени, правка, glTF (`cutscenes.md`) |
| Диалоги F2F | `dlg`, `chv` | да: `dialogue`, `dialogue-import`, `subtitle`, GUI (`dialogues.md`) |
| Субтитры роликов | `tob`; `TOB1` в звуке ролика | чтение и запись (§5.1, `cutscenes.md` §6) |
| События речи | `get` | нет, формат простой (§5.2) |
| Очки и достижения | `score` | нет |
| Маршруты ИИ | `graph` | да: `graph-info`, `graph-export`/`graph-import`, `graph-edit`, слой во вкладке уровня (`navigation-graph.md`, этап 42) |

## 5. Небольшие типы

### 5.1 `tob` — тайминги субтитров роликов [П, все 15 ресурсов]

| Смещение | Тип | Поле |
|---|---|---|
| 0x00 | 4 байта | `TOB1` |
| 0x04 | u32 | размер ресурса |
| 0x08 | u32 | версия = 1 |
| 0x0C | u32 | `sdbm` имени ролика в нижнем регистре (`fmv01`, `trn01`…`trn16`, `deadspace_trailer` — файлы `movies/<язык>/*.vp6`) |
| 0x10 | u32 | число групп G (1 или 2) |
| 0x14 | G × 12 | группа: `u32 sdbm(имя)`, `u32 число записей`, `u32 первая запись` |
| 0x14 + 12·G | N × 16 | запись: `u32 хэш строки lh2`, `u32 начало, мс`, `u32 длительность, мс`, `u32 0` |

Группы — `sdbm("subtitles")` = `0x8ACF265B` и `sdbm("titles")` = `0x32B9423B`;
их отрезки идут подряд и покрывают все записи. Внутри группы записи упорядочены
по началу. Хэш строки — ключ таблицы `lh2` (`0xCAE079FD` → `fe_movies_01`).

### 5.2 `get` — таблица событий речи [П по структуре, Г по смыслу]

Один ресурс (`globals_global`, 49 400 байт): `u16 версия = 3`, `u16 N = 1372`,
`u32 смещение записей = 8`, затем N записей по 36 байт, ключи уникальны и
отсортированы. Запись: `u32 sdbm(lower(имя события))` (`CREW_IM_DOWN_MEDIC`,
`F2F_TAKEOVER_START`, `PHONE_*` …), затем f32 вес, f32 и f32 задержки повтора
(−1 — нет), u32, f32 (приоритет или громкость), u32 категория, u32 флаги, u32.

### 5.3 `score`, `trinity_seq_master`, `dlg`, `chv`

- `score` (2 одинаковых ресурса по 63 129 байт): версия 10, таблица чисел и
  смещений, 4 кривые по 8 пар f32, записи по 20 байт и около 1200 строк-имён
  сообщений (`iMsgPlayerArrested`, `iMsgRankIncrease`, `give_*_favor`).
  **[Г]** база событий счёта и достижений, подписанная на шину сообщений.
- `trinity_seq_master` (260): кадры роликов на движке — список команд
  секвенсора; разобраны полностью, см. `cutscenes.md`.
- `dlg` (143) — деревья диалогов F2F с анимациями `f2f_*`; `chv` (259) —
  события речи персонажа, на них ссылается `ChatterComponent`. Разобраны
  полностью: `dialogues.md`.

## 6. Открыто

- Используется ли состояние `+0x1C`.
- Смысл атрибутов 1–2 у `MultiManager` (по коду: 1 — число срабатываний,
  −1 у 9 015 объектов; 2 — флаги) и 6, 8–10 у `TriggerBase` — названия
  этапа 27 помечены `guess`.
- Хэш и шина у 12 классов, которые хэшируют строку не в читателе
  (`AndEventGate` 0–3, `MessageRelay` 0, `ForwardSender` 0, …), — по данным.
- Как таймеры вызывают функцию Lua по имени (по поведению — глобальная функция с этим именем, проверено этапом 28).
