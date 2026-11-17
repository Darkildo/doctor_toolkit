# Визуальные скрипты (Blueprints → Lua)

Этап 28. Свой скрипт для The Godfather II (PC) собирается из узлов, как
Blueprint в Unreal: события, ветвления, задержки, значения, действия.
Граф хранится в JSON, компилируется в читаемый Lua 5.0 и в байт-код игры, а
в уровень попадает как ресурс `lua` и объект SimGroup `LuaAudioScript`,
который его запускает. Скрипт слушает и посылает игровые сообщения, читает
и пишет переменные игры, работает с таймерами — то есть встраивается в
логику миссий (`docs/formats/mission-logic.md` §4) наравне с триггерами и
`MultiManager`.

Код: `crates/doctor-toolkit/src/script/` (`graph.rs` — модель и JSON,
`library.rs` — узлы, `compile.rs` — компилятор, `sim.rs` — макет API игры,
`deploy.rs` — размещение в уровне), `resource/lua/vm.rs` — виртуальная
машина Lua 5.0 для проверки, CLI `script-*` (`crates/doctor-toolkit-cli/src/script_cmd.rs`).

## 1. Что проверено в игре

Стенд (`docs/analysis/game-bench.md`), теневая копия, новая игра до террасы
в Гаване. Сигнал — файловые пробы: `loadfile("doctor_probe_<метка>.lua")`
ищет файл в каталоге игры, а `strace` стенда (`TRACE_FILES=1`,
`TRACE_SYSCALLS=openat,newfstatat,statx,…`) видит обращение к
несуществующему файлу (`statx … ENOENT`). Метка несёт значения (`a_var_after_2`),
поэтому видно не только «дошло», но и «что вернула функция».

Этап 28 (`game-bench.md` §14): ресурс `lua` и объект `LuaAudioScript`
в SimGroup уровня запускаются при загрузке и по сообщению игры; скрипт
получает сообщения (`audio_init`…), его `SendMessage` доходит до объектов
SimGroup (`MessageRelay`, `VariableSwitch`, `VariableOperator` отвечают),
переменные игры читаются и пишутся, таймеры срабатывают; граф
`examples/scripts/doctor_e2e.json` проходит все пробы.

Этап 45 (`game-bench.md` §20), старт `-stream gf2 cuba`:

- **видимое действие и триггер:** `TriggerBox` (копия объекта задания c67,
  `level add --template-stream`, `TriggerBase.4 = doctor_trig_enter`) в 6 м
  перед игроком и граф «On message `doctor_trig_enter` → File probe → Send
  message `boundary_fade_out`» (`script-deploy … gf2/cuba`): игрок дошёл до
  коробки — проба `trig_enter` в трассировке, экран погас (средняя яркость
  кадра 106,5 → 1,6);
- **звук:** `PlayEmx("doctor_beep.emx", 1)` нового эмиттера `sound-add
  --new` слышен в записи (`AUDIO_CAPTURE=1`); **имя нужно с `.emx`** —
  `PlayEmx("doctor_beep")` и даже `PlayEmx("cloth_swipe_quick_01")` игра не
  находит (возвращает нулевой указатель). Узел Play sound поэтому дописывает
  `.emx` сам (`dt_emx`).

Этап 74 (`game-bench.md` §35, `docs/analysis/lua-api.md`): функции Lua
игры вызваны в игре напрямую (`live lua`) и узлами графа `doctor_s74n`:
данные сообщений (число, посланное скриптом, читается обратно), поток
`PlayEarsAudioStream` слышен в записи, стоповое сообщение скрипта вызывает
«On stop» и останавливает таймеры, время и переменные приходят числами,
узлы групп с ненайденным объектом игру не роняют. Там же найдено, что
`Base` от строки объектов игры не находит, а `GetPlayerPosition` требует
номер игрока.

## 2. Граф

```json
{
  "format": "doctor-toolkit-script", "version": 1,
  "name": "hello",
  "start": { "on_load": false, "message": "audio_init_delay" },
  "variables": [{ "name": "count", "type": "number", "default": 0 }],
  "nodes": [
    { "id": 1, "kind": "event.message", "pos": [0, 0], "params": { "message": "doctor_ping" } },
    { "id": 2, "kind": "action.send_message", "pos": [260, 0], "inputs": { "message": "doctor_pong" } }
  ],
  "links": [{ "from": [1, "then"], "to": [2, "exec"] }]
}
```

- `name` — имя скрипта: буквы, цифры, `_`; чанк называется `<name>.lua`.
- `start` — как скрипт запускается в уровне: `on_load` (бит 0 атрибута 0
  объекта `LuaAudioScript`), `message` (атрибут 1), `stop_message` (атрибут 2).
- `variables` — переменные скрипта (живут, пока жив скрипт), типы `number`,
  `bool`, `string`, `position`, `group`, `any`.
- У узла `params` — настройки (имя сообщения, таймера), `inputs` — значения
  входов без провода. Пины по имени; набор пинов задаёт вид узла и его
  настройки (у `flow.sequence` — число выходов, у функций — список входов).
- `links` — провода «выход → вход». Выход exec ведёт в одно место (несколько
  — через Sequence), вход данных принимает один провод; выход данных можно
  вести куда угодно.

Пины типизированы: `exec` (порядок выполнения), `number`, `bool`, `string`,
`position`, `group`, `any`. Число и логическое значение сами превращаются в
текст (`tostring`), `any` соединяется с чем угодно.

JSON пишется детерминированно (узлы по id, провода по порядку), поэтому
граф удобно держать в git.

## 3. Узлы

`doctor-toolkit script-nodes` печатает библиотеку (с `--json` — машинно
читаемо). «Не проверено» — функции API, чьи аргументы выведены из скриптов
игры, но вызов из нашего скрипта в игре не наблюдался.

### События

| Вид | Узел | Выходы | Настройки | Что делает |
|---|---|---|---|---|
| `event.load` | On start | `then` | — | при запуске скрипта (загрузка уровня или стартовое сообщение) |
| `event.stop` | On stop | `then` | — | при стоповом сообщении (`shutdown`) |
| `event.message` | On message | `then`, `data:any`, `type:string` | `message` | пришло игровое сообщение; данные — что приложил отправитель |
| `event.timer` | On timer | `then` | `name`, `seconds`, `repeat`, `autostart` | каждые `seconds` секунд (или один раз) |
| `event.function` | Function | `then` + входы функции | `name`, `inputs` | функция скрипта; вызывается узлом Call function |

### Порядок выполнения

| Вид | Узел | Входы | Выходы | Что делает |
|---|---|---|---|---|
| `flow.branch` | Branch | `exec`, `condition:bool` | `true`, `false` | если |
| `flow.sequence` | Sequence | `exec` | `then0…thenN` (`count`) | выходы по очереди |
| `flow.delay` | Delay | `exec`, `seconds:number` (1) | `then` | продолжить через `seconds` (таймер игры); новый вход во время ожидания начинает ожидание заново |
| `flow.for` | For loop | `exec`, `first`, `last` | `body`, `index:number`, `done` | цикл по целым числам |
| `flow.for_each_member` | For each member | `exec`, `group:group` | `body`, `member:any`, `done` | по членам группы (`ForEachMember`) |
| `flow.gate` | Gate | `enter`, `open`, `close`, `toggle` | `exit` | пропускает, пока открыт (`start_open`) |
| `flow.do_once` | Do once | `exec`, `reset` | `then` | только первый раз до `reset` |
| `flow.flip_flop` | Flip-flop | `exec` | `a`, `b`, `is_a:bool` | по очереди A и B |
| `flow.call_function` | Call function | `exec` + входы функции | `then` | вызвать Function скрипта (`function`) |

### Значения, математика, логика, текст (чистые узлы)

| Вид | Что даёт |
|---|---|
| `value.number`, `value.bool`, `value.string` | константа из настройки `value` |
| `math.add`, `math.sub`, `math.mul`, `math.div`, `math.mod`, `math.min`, `math.max` | `a ∘ b` (`math.mod` — остаток) |
| `math.abs`, `math.floor`, `math.negate` | функция от `x` |
| `math.clamp` | `x`, ограниченный `min…max` |
| `math.random` | целое от `min` до `max` (`math.random`) |
| `compare.eq`, `compare.ne` | `a == b`, `a ~= b` (любые значения) |
| `compare.lt`, `compare.le`, `compare.gt`, `compare.ge` | сравнение чисел |
| `logic.and`, `logic.or`, `logic.not` | логика |
| `string.concat` | `a .. b` |
| `string.from` | `tostring(value)` |
| `string.number` | число с `decimals` знаками (`string.format("%.Nf")`) |
| `string.length` | длина в байтах |

### Переменные

| Вид | Узел | Что делает |
|---|---|---|
| `var.get` | Get variable | значение переменной графа (`variable`) |
| `var.set` | Set variable | присвоить; выход `value` — новое значение |
| `engine.get` | Get game variable | `Int32ToNumber(VariableGet(FindVariable(name)))` — переменная игры по имени (`sdbm`); если нет — создаётся со значением 0. `VariableGet` отдаёт объект `Int32`, а не число: без приведения сравнение с числом ложно или даёт ошибку (этап 74) |
| `engine.set` | Set game variable | `VariableSet(FindVariable(name), value)`; переменные, созданные так, — **целые**: дробное отбрасывается, `true`/`false` → 1/0 (иначе игра отказывает: «Could not turn (null) into an Int32») |

### Игра (запросы)

| Вид | Вызов | Примечание |
|---|---|---|
| `game.sim_time` | `Int32ToNumber(GetSimTime()) / 1000` | в игре миллисекунды объектом `UInt32` (деление такого объекта целое, склейка со строкой — ошибка); узел приводит к числу и даёт секунды игрового времени |
| `game.player_position` | `GetPlayerPosition(0)` | номер игрока обязателен: без него функция всегда даёт `nil` (до этапа 74 узел звал её без аргумента); `nil` и пока игрока нет; `GetPositionValues(nil)` даёт 0, 0, 0 |
| `game.position_xyz` | `GetPositionValues(p)` | x, y (вверх), z |
| `game.distance`, `game.distance_to` | — | расстояние между позициями / до точки (прелюдия) |
| `game.foot_police`, `game.car_police` | `GetFootPoliceIntensity()`, `GetCarPoliceIntensity()` | по коду обе дают только `nil` или 0 (силы погони не возвращают); без погони — `nil`; с погоней не проверено |
| `game.vehicle_speed`, `game.vehicle_type` | `GetVehicleSpeed()`, `GetVehicleType()` | пешком 0 (проверено); за рулём не проверено |
| `game.in_vehicle` | второй результат `GetVehicleSpeed()` | пешком `false` (проверено) |
| `game.vehicle_speed_delta`, `game.vehicle_acceleration`, `game.vehicle_skid`, `game.vehicle_health` | `GetVehicleSpeedDelta()`, `GetVehicleAcceleration()`, `GetVehicleSkidEnergy()`, `GetVehicleHealth()` | пешком 0 (проверено) |
| `object.from_guid`, `object.valid`, `object.guid` | `Base(guid)`, `tostring(object) ~= nil`, `tostring(object)` | ссылка на объект по GUID `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`. В игре разбор GUID сломан (`lua-api.md` §3): объекты игры не находятся, ссылка пустая, `object.valid` — `false` |
| `game.platform` | `GetPlatform()` | в игре — `"windows"` |

### Группы

`group.find` (`GroupManager:FindGroup`), `group.register`
(`GroupManager:RegisterGroup`), `group.add`/`group.remove`
(`AddMember`/`RemoveMember`), `group.clear`, `group.count`
(`Int32ToNumber(group:GetMemberCount())`), `group.contains`,
`flow.for_each_member`.

Этап 74 (`docs/analysis/lua-api.md` §3–§4): члены группы — объекты игры
(`Base`), а взять объект игры скрипту неоткуда: `Base` от строки в этой
сборке не находит объектов. Вызов `AddMember`, `RemoveMember` или `IsMember`
с ненайденным объектом **роняет игру** (чтение по нулю, `0x00469F85`),
поэтому узлы `group.add` и `group.remove` сначала проверяют ссылку и для
пустой ничего не делают. `IsMember` и `Clear` в exe — заглушки; узлы
`group.contains` и `group.clear` сделаны через `ForEachMember`. На стенде
проверены пустая группа (создание, поиск, счётчик, обход) и то, что узлы с
ненайденным объектом игру не роняют; группа с настоящим объектом — нет.

### Сообщения, таймеры, звук, Lua, отладка

| Вид | Узел | Что делает |
|---|---|---|
| `action.send_message` | Send message | `SendMessage(message)` — то же сообщение, что шлют триггеры и `MultiManager`. В редакторе у поля сообщения (и у «On message») кнопка `▾`: имена сообщений движка и файлов игры с набранным текстом (этап 73, `docs/logic-graph.md` §2.1); любое другое имя — новое сообщение |
| `action.send_message_data` | Send message with data | `SendMessage(message, type, data)`: слушатель получает указатель на число, текст или позицию; `type` — любой непустой текст, без него данные не уходят (проверено) |
| `message.data_number` | Number of message data | число, посланное узлом выше, из выхода `data` события On message (`GetPositionValues(data)`, первое значение; проверено) |
| `action.query` | Ask the game | `SendMessageEx(message, data)` → до трёх ответов; `iMsgLuaGetPlayerVenue` на террасе отвечает пустой строкой, у ворот аэропорта — строкой `"4"` |
| `timer.start`, `timer.stop` | Start/Stop timer | управление событием On timer по имени |
| `audio.play_emx`, `audio.stop_emx`, `audio.emx_volume` | звук-эмиттер `.emx` | `PlayEmx(name .. ".emx", volume)`: игра ищет звук по имени файла с `.emx`, узел дописывает его (этап 45, проверено с записью звука) |
| `audio.play_stream`, `audio.stop_stream` | поток `.stx` | `PlayEarsAudioStream(name .. ".stx", volume)`, `StopEarsAudioStream(handle)`: игра ищет поток по имени с `.stx`, узел дописывает его (этап 74, проверено с записью звука) |
| `audio.prime_stream`, `audio.start_stream` | подготовить и запустить | `PrimeEarsAudioStream(name, volume)` загружает поток молча, `PlayEarsAudioStream(handle)` запускает (проверено) |
| `audio.stream_stop_message` | Message when stream stops | `SetEarsAudioStreamStopMessage(handle, message)`: сообщение приходит после `StopEarsAudioStream` (проверено); при естественном конце потока — не проверено |
| `audio.handle_valid` | Sound found | найден ли звук или поток: ненайденный — light userdata 0, истинное в Lua значение; узел сравнивает `tostring(handle)` с `userdata: 00000000` |
| `audio.apply_mix`, `audio.remove_mix` | снимок микшера `.mix` | `ApplyMix(name, true)`, `RemoveMix(name)`; не проверено |
| `lua.call` | Call game function | любая глобальная функция Lua игры (`function`, `args`, `results`) |
| `lua.code` | Lua code | свои операторы Lua 5.0; входы — `{имя}`, выходы — `{имя} = …` |
| `debug.probe` | File probe | `loadfile("doctor_probe_<tag>.lua")` — метка в трассировке файлов |
| `comment` | Comment | заметка на холсте, в код не попадает |

## 4. Компилятор

`script::compile(graph)` → `Compiled { source, chunk, diagnostics, line_nodes }`.
Никогда не падает: всё, что не так, — диагностики с номером узла и пина.

Что получается (пример — `doctor-toolkit script-compile graph.json`):

```lua
-- hello.lua: made by doctor_toolkit from the visual script "hello".
-- Edit the graph, not this file: it is generated again.

local SCRIPT = "hello.lua"
local G = "dts_hello_"
local V, S, K, F, E = {}, {}, {}, {}, {}
…                                    -- прелюдия: dt_run, dt_on, dt_timer, …

-- [1] On message: doctor_ping
E.n1 = function(data, dtype)
  S.n1_data = data
  S.n1_type = dtype
  -- [2] Send message
  SendMessage("doctor_pong")
end

RegisterFunction(SCRIPT, "startup", function()
  dt_on("doctor_ping", E.n1)
  dt.receiver = NewMessageReceiver(dt_receive)
  for message in pairs(dt.handlers) do
    dt.receiver:AddMessage(message)
  end
end)

RegisterFunction(SCRIPT, "shutdown", function() … end)
```

Правила:

- **Событие — функция** `E.n<id>`; цепочка exec-узлов за ним — операторы по
  порядку, у каждого комментарий `-- [id] Название`.
- **Чистые узлы** (математика, сравнения, запросы) — выражения, подставляемые
  туда, где используется их выход; вычисляются при каждом использовании.
- **Выходы exec-узлов** хранятся в таблице `S` (`S.n12_handle`): их видят
  узлы дальше по цепочке и другие события. Переменные графа — в `V`.
- Вход exec, в который ведут несколько проводов, и продолжение **Delay** —
  функции в `K`. Delay — одноразовый таймер игры: `CreateTimer` вызывает
  глобальную функцию по имени, поэтому у каждого таймера скрипта есть
  глобальный обработчик `dts_<скрипт>_<ключ>` (префикс отделяет скрипты
  друг от друга: все скрипты игры живут в одном состоянии Lua).
- **Ошибка в обработчике** останавливает только его (`pcall` в `dt_run`).
  С `"trace_errors": true` ошибка оставляет файловую пробу
  `doctor_error_<скрипт>_<где>_<текст>` — так ошибки видны в трассировке
  стенда (в игре `print` и ошибки теряются).
- Ограничения Lua 5.0 соблюдаются: в главной функции мало локальных
  переменных (меньше 32 upvalue на функцию), строка, начинающаяся со скобки,
  не порождает «ambiguous syntax» (`dt_self`).
- Байт-код — компилятором этапа 16 (`lua::compile_for_game`, числа float).

Проверки (ошибки помечают узел): неизвестный вид узла, провод к
несуществующему пину или узлу, несовместимые типы, два провода из одного
exec-выхода или в один вход данных, пустое имя сообщения, таймер без
секунд, ссылки на несуществующие таймер, функцию и переменную, плохие имена,
цикл проводов данных, **цикл exec без Delay** (бесконечная рекурсия в игре),
пустые входы `position`/`group`, синтаксис узлов `lua.code` (ошибка Lua
сопоставляется узлу по таблице строк). Предупреждения: узел, до которого не
доходит ни одно событие.

## 5. Проверка без игры: виртуальная машина и макет API

Lua 5.0 на Rust в проекте не было. Варианты и выбор:

| Вариант | Почему нет / да |
|---|---|
| Только структурные тесты (текст на месте) | не ловят ошибок семантики: порядка, upvalue, таймеров |
| Свой интерпретатор подмножества (по AST) | проверял бы не то, что исполняет игра: AST ≠ байт-код нашего компилятора |
| Декомпилятор + интерпретатор | два слоя, которые сами нуждаются в проверке |
| **Виртуальная машина байт-кода Lua 5.0** | исполняет ровно тот чанк, что уйдёт в игру; 35 опкодов, около 1000 строк |

Выбрана ВМ (`resource/lua/vm.rs`): все опкоды Lua 5.0, замыкания и upvalue,
`...` как таблица `arg`, `pcall`/`error`, метатаблицы с `__index`, числа
округляются до `float` после каждой операции, `tostring` — `%.14g` (как в
игре: `tostring(0.1)` = `0.10000000149012`, проверено пробой), библиотеки
base, string, table, math, coroutine (сопрограммы — этап 74; без полных шаблонов строк).
Сверка с настоящим Lua 5.0.3 (сборка с `float` в рабочей папке проверки;
у неё `LUA_NUMBER_FMT` = `%.7g`, поэтому ВМ запускалась с
`script-run --number-digits 7`): тестовые программы дистрибутива из
подмножества (`bisect`, `cf`, `factorial`, `sort`) дают побайтно тот же
вывод; остальные используют `io`, `os`, `setfenv` — вне подмножества
(сопрограммы появились позже, на этапе 74, и сверены с игрой, а не с этой
сборкой: `docs/analysis/lua-api.md` §10).

Макет игры (`script::sim::Sim`) регистрирует API игры: сообщения и
получатели (`NewMessageReceiver` с `AddMessage`…), таймеры (`CreateTimer`
вызывает глобальную функцию по имени, время идёт по `advance`),
переменные (целые, как в игре), позиции, объекты (`Base`) и группы, звук (журнал),
`RegisterFunction`/`CallLuaFunction`, `loadfile` (журнал проб), реле
сообщений вместо объектов SimGroup. Всё, что скрипт сделал с «игрой»,
пишется в журнал (`SimEvent`).

Тесты (`script/tests.rs`): сообщения, ветвления и переменные, таймеры и
задержки (время срабатываний), цикл через Delay, Gate, Do once, Flip-flop,
переменные игры и позиции, циклы For, функции, свой Lua, вызов функций игры,
диагностики, узел каждого вида. Развёртывание — `script/deploy_tests.rs`
(миниатюрная игра) и корпусный тест
`visual_scripts_deploy_update_and_remove_in_a_workspace`.

## 6. Размещение в уровне

`script::deploy::deploy(game, index, ws, stream, graph, chunk)` — одна
операция рабочего пространства (отмена — Ctrl+Z / `workspace undo`):

1. ресурс `lua` с новым id (`id × 4`, как у скриптов игры; id не занят ни
   одним ресурсом игры) — в поток **перед его SimGroup**;
2. объект `LuaAudioScript` — копия объекта `gf2/globals_global` — в конец
   SimGroup потока, атрибуты: 0 — флаги (бит 0 — запуск при загрузке),
   1 — стартовое сообщение, 2 — стоповое, 3 — GUID ресурса.

Повторное размещение находит скрипт по имени чанка (`<name>.lua`) и меняет
данные ресурса и настройки того же объекта — до и после `workspace apply`.
`remove` убирает объект и ресурс. Граф кладётся рядом с рабочим
пространством: `<workspace>/scripts/<name>.json`.

Куда класть:

| Поток | Когда работает |
|---|---|
| `gf2/globals_global` | всегда в игре (загружается с любым уровнем; там же 29 скриптов звука игры) |
| `gf2/cuba`, `gf2/miami`, `gf2/queens` и их `local` | в своём городе |
| поток задания (`gf2/<город>/global/objectives/…/cp_NNN…`) | пока загружен этап миссии |
| `gf2/frontend*` | **нельзя**: в главном меню нет симуляции и состояния Lua, объект с найденным скриптом роняет игру (`0x00480E03`); инструмент отказывает |

Запуск. `start.on_load` — объект запускает скрипт, когда читается SimGroup
(загрузка уровня); `start.message` — по сообщению (например `audio_init` или
`audio_init_delay`: их шлют объекты `TriggerAuto` из `gf2/globals_global`
сразу после загрузки; так запускаются скрипты звука самой игры). События
*On start* выполняются на первом такте игры после запуска: скрипт,
запущенный при чтении SimGroup, иначе слал бы сообщения объектам, которых
ещё нет (§1). `start.stop_message` вызывает `shutdown`: таймеры
останавливаются, получатель сообщений удаляется, выполняются *On stop*.

Скрипты живут в общем состоянии Lua игры (`[LuaManager+0x18]`, создаётся с
симуляцией): глобальные имена у каждого скрипта свои (`dts_<скрипт>_…`),
остальное — локальные переменные чанка.

## 7. Командная строка

```sh
T=./target/release/doctor-toolkit
GAME="/path/to/The Godfather II"

$T script-nodes                                  # библиотека узлов
$T script-compile hello.json -o hello.lua --chunk hello.luac
$T script-run hello.json send:doctor_ping wait:2.5 var:doctor_var=3 stop
$T script-run any.lua                            # обычный Lua против макета игры
$T script-deploy "$GAME" gf2/globals_global hello.json --start-message audio_init_delay
$T script-list "$GAME" gf2/globals_global
$T script-remove "$GAME" gf2/globals_global hello
$T workspace apply "$GAME"
```

`script-run` печатает, что сделал скрипт: `send`, `probe`, `set`, вызовы
функций игры, срабатывания таймеров; шаги — `send:СООБЩЕНИЕ`,
`wait:СЕКУНДЫ`, `var:ИМЯ=ЧИСЛО`, `player:X,Y,Z`, `relay:ИЗ=В`
(объект-реле, как `MessageRelay`), `stop`. `--json` — журнал строками JSON.

Примеры графов — `examples/scripts/`.

## 8. Редактор в GUI

**Tools → Script editor** — окно редактора на общем виджете графа узлов
(`node_graph`, этап 27):

- слева — палитра узлов по категориям с поиском (щелчок добавляет узел);
- в центре — граф: провода между типизированными пинами (цвет — тип,
  белые — exec), правый щелчок — «Add node» по категориям, вставка,
  комментарий вокруг выделения, удаление и дубликат узла, разрыв провода;
  Delete, Ctrl+C / Ctrl+V, F, Home, мини-карта; **Auto layout** —
  раскладка слоями, как у графа логики;
- справа — выбранный узел: описание, его ошибки, настройки и значения
  входов без провода; без выбора — настройки скрипта (запуск при загрузке,
  стартовое и стоповое сообщения, пробы ошибок, описание, переменные);
- снизу — **Problems** (щелчок выделяет узел), **Lua** (сгенерированный
  текст), **Dry run** (шаги `send:`, `wait:`, `var:`, `player:`, `relay:`,
  `stop` на макете игры и журнал того, что сделал скрипт).

Граф перекомпилируется при каждой правке: ошибки — красные значки на
узлах, предупреждения — жёлтые, «?» — узел, не проверенный в игре.
**Deploy to level** (поток уровня в поле рядом, по умолчанию
`gf2/globals_global`) записывает скрипт в рабочее пространство одной
операцией (Ctrl+Z в главном окне отменяет), **Remove from level** убирает.
Open / Save / Save as — файлы `.json`; размещённый граф сохраняется и в
`<workspace>/scripts/`. Отмена правок графа — Undo/Redo окна (Ctrl+Z над
холстом). `DOCTOR_TOOLKIT_GUI_SCRIPT=<graph.json>` открывает редактор с
графом при запуске (проверка без пользователя).

### 8.1 Журнал скрипта в игре и точки останова (этап 35)

Флажок **«Отмечать узлы в игре»** (`"trace_nodes": true` в JSON графа):
компилятор ставит первой строкой каждого узла с exec-входом и каждого
события `print("dt:<скрипт>:<узел>;")` (`compile::trace_text`,
разбор — `parse_trace`); строка принадлежит своему узлу в таблице строк,
так что и ошибка в ней указывает на узел. Lua `print` игры через перехват
модуля живой связи попадает в журнал со скриптом и строкой
(`docs/analysis/live-link.md` §11.1). Без флажка код прежний.

Вкладка **«Игра»** внизу редактора (при живой связи): какие узлы сработали
и когда, ошибки Lua в чанке `<скрипт>.lua` — с узлом по строке ошибки (или
по первой строке стека в этом чанке); щелчок выделяет узел. На графе
сработавшие узлы вспыхивают зелёным, над ними «×N»; упавшие — красной
рамкой. У выделенного узла — «сработал в игре: N» и **«Остановить игру на
этом узле»**: точка останова модуля на тексте `dt:<скрипт>:<узел>;` —
игра встаёт в `print` этого узла (если узел сработал от сообщения,
посланного самой связью, — в следующем кадре), вкладка показывает «Игра
стоит на узле N» и «Продолжить».

Стенд (`game-bench.md` §22): граф `doctor_dbg` (On start → Send message,
On message `doctor_dbg_ping` → Send message) с отметками в
`gf2/cuba/local`: при загрузке в журнале `dt:doctor_dbg:1;` и `:2;`
(`doctor_dbg.lua:97`, `:98`), на каждый `live send doctor_dbg_ping` —
`:3;` и `:4;`; точка на узле 4 остановила игру, редактор показал «×1»,
«×7» и «Игра стоит на узле 4».

## 9. Открыто

Этап 74 (`docs/analysis/lua-api.md`, стенд — `game-bench.md` §35) закрыл
`PlayEarsAudioStream`, данные сообщений, `shutdown` и сопрограммы; осталось:

- `SetTimeOfDay` и `UIFanfare` по отдельности в кадре не сверялись
  (видимое действие от сообщения скрипта проверено на этапе 45).
- Объект игры скрипту взять неоткуда (`Base` от строки сломан); группы с
  настоящим объектом не проверены. Догадка: объект мода с GUID особого вида
  `Base` найдёт (`lua-api.md` §3).
- Полиция при погоне и транспорт за рулём не проверены.
- Что лежит по указателю в данных каждого сообщения игры.
- Как аргумент громкости потока переводится в громкость; стоповое
  сообщение при естественном конце потока.
- Макет API отдаёт время, переменные и число членов группы обычными
  числами, а игра — объектами `UInt32`/`Int32`.
