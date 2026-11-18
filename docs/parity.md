<!-- Написано командой `doctor-toolkit parity-gen` из data/toolkit/capabilities.toml. Руками не править. -->
# Паритет поверхностей: что где доступно

У тулкита четыре поверхности: MCP-сервер для агентов, окно редактора, командная строка и привязки Python. Все они зовут одну библиотеку и пишут в одно рабочее пространство; расходится только то, что на каждой из них выведено наружу. Здесь — каждая возможность тулкита и где она есть.

Источник — таблица `data/toolkit/capabilities.toml`. Тесты сервера MCP, редактора и командной строки падают, если инструмент, команда меню, мастер или команда CLI не упомянуты ни в одной строке, если строка называет то, чего в коде нет, или у строки нет ни одной поверхности. Новая возможность начинается со строки в таблице; этот документ обновляет `doctor-toolkit parity-gen`.

Как читать:

- **MCP** — инструменты сервера ([MCP](mcp.md)). Знак ᴱ — инструмент есть только с `--allow-edits`, ᴳ — только с `--allow-game`.
- **Редактор** — команда меню или палитры команд в кавычках (её английское название), мастер, либо путь по панелям и кнопкам ([оболочка](shell.md), [руководство](guide/README.md)). «Только мышью» — действие нельзя задать полями, клавишей или предустановкой. «Без рук» — предустановка `DOCTOR_TOOLKIT_GUI_…` (приставка опущена), которой действие делается без человека.
- **CLI** — команда `doctor-toolkit …`. **Python** — имя из модуля `doctor_toolkit` ([Python](python.md)).
- «—» — поверхности у возможности нет. «Частично» — есть не всё, и сказано, чего не хватает. «Намеренно нет» — не будет, и сказано почему.

## Числа

Возможностей в таблице: **149**.

| Поверхность | Полностью | Частично | Нет | Намеренно нет |
|---|---|---|---|---|
| MCP | 130 | 5 | 0 | 14 |
| Редактор | 141 | 0 | 0 | 8 |
| CLI | 131 | 5 | 13 | 0 |
| Python | 52 | 1 | 96 | 0 |

- Есть в MCP, нет в редакторе: **0**.
- Есть в редакторе, нет в MCP: **0**.
- Есть в обоих, но в редакторе частично: **0**; в MCP частично: **5**.
- Нет ни в MCP, ни в редакторе — только командная строка: **0**.
- Строк с «намеренно нет»: **15**.
- В редакторе только мышью: **3**.

## Сводная таблица

### Игра: обзор, поиск, навигация

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Открыть игру и получить обзор: профиль, потоки, типы ресурсов, области, миссии | `game_info` | «Open game»; экран Welcome | `types` — **частично**: только таблица типов ресурсов; обзора областей и миссий одной командой нет | `Game.new`, `Game.profile`, `Game.root` |
| Найти установленные копии игры и их издание | `detect_games` | «Find installed games»; экран Welcome → Search again | `detect` | — |
| Список потоков и таблиц содержимого | `list_streams` | «Open stream», «Files panel» | `toc`, `info` | `Game.streams`, `Game.stream_files`, `Game.tocs`, `Game.toc`, `Game.has_stream` |
| Список ресурсов потока | `search`; фильтр stream: | вкладка потока: таблица ресурсов. Без рук: `SELECT=N выбирает ресурс по номеру (по id или имени — нет)` | `list` | `Game.stream`, `Stream.resources`, `Game.resources` |
| Поиск по всей игре: имя, шаблон, хеш, ключ, тип, поток, текст строк | `search` | «Search the game», «New search tab», «All resources», «Command palette», «Content Drawer», «New Content Browser». Без рук: `SEARCH=<запрос>; SHELL=palette=<текст>; CONTENT=<место>` | `search` | `Index.search`, `Index.count`, `Index.keys` |
| Построить или обновить индекс игры | строится сам при запуске сервера | «Rebuild game index». Без рук: `SHELL=index` | `index` | `Game.index`, `Index.refresh` |
| Сведения о ресурсе: имена, хеши, что использует и кто использует, копии | `resource_info` | «Details panel», «Another Details panel», «Back», «Forward»; панель Details: Refers to / Referred to by | `refs` | `Index.refs`, `Index.record`, `Game.resource` |
| Содержимое ресурса по его типу (дерево, JSON, исходник) | `resource_json` | вкладка ресурса: просмотрщик по типу | `logic-export`, `skeletons`, `fonts` — **частично**: только файлами; на экран — свои команды по типам (graph-info, sbk-info, dialogue, partsdb show…) | `Resource.parse`, `Resource.to_dict`, `decode` |
| Имена хеша и хеши имени под каждой хеш-функцией движка | `lookup_hash` | «Hashes»; панель Hashes: имя под каждой хеш-функцией, имена хеша, где хеш встречается; поиск и палитра находят ресурсы по хешу. Без рук: `SHELL=hash=<имя или хеш>` | `hash`, `lookup` | `hash_all`, `lookup`, `sdbm`, `crc32`, `fnv1a`, `Index.lookup` |
| Список хешей игры без известного имени | **намеренно нет**: исследовательская сводка для пополнения словаря имён тулкита, а не действие над игрой или модом | **намеренно нет**: то же: нужна разработчику тулкита, не автору мода | `unknown-hashes` | — |
| Сырые данные ресурса: сохранить, посмотреть в hex | `export`; format: raw | панель Details → Save data…, Load hex | `extract`, `dump` | `Game.read`, `Resource.read`, `Stream.read` |

### Ресурсы и потоки

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Экспорт ресурса в обычный формат: PNG/DDS, glTF/OBJ/FBX/USD, WAV, JSON, Lua | `export` | «Export»; меню ресурса и Content Browser → Export; кнопки Export в просмотрщиках | `textures`, `models`, `logic-export`, `text-export`, `audio`, `fonts`, `skeletons`, `lua-decompile`, `lua-export`, `collision-export`, `graph-export`, `dialogue-export`, `apx-export` | `Texture.to_png`, `Texture.to_dds`, `Model.to_glb`, `Model.export`, `Sound.to_wav`, `Workspace.export_model` |
| Распаковать потоки в папку файлов и записать изменённые файлы обратно | `unpack`, `pack`ᴱ; unpack пишет в папку вывода сервера (--output-dir); pack — одна операция | «Unpack selected», «Pack folder into workspace» | `unpack`, `pack` | `Game.unpack`, `Game.pack` |
| Вернуть отредактированный экспорт: JSON строк, логики, графа, диалога, исходник Lua, сырые данные | `import_file`ᴱ | меню Asset ресурса (вкладка, контекстное меню вкладки, Details): «Replace with a file…» — разбор по типу, как import_file (JSON строк, логики, графа навигации, диалога, исходник или байт-код Lua), и «Replace with raw data…»; кроме того Import JSON в просмотрщиках, Load… у Lua, Save у таблицы строк. Без рук: `RESOURCE=import=<файл> \| raw=<файл>` | `text-import`, `logic-import`, `graph-import`, `dialogue-import`, `lua-import` | `Workspace.set_data`, `Workspace.set_many`, `Workspace.set_string_table` |
| Добавить в поток новый ресурс из файла сырых данных | `add_resource`ᴱ | «Add resource from a file»; окно «New zone: streams and resources», форма Add resource; Content Browser → контекстное меню потока → Add a resource from a file here…. Без рук: `ZONE=resource:stream=,type=,file=[,ids=][,policy=][,compress]` | `add-resource` | — |
| Клонировать ресурсы под новыми id: в тот же или другой поток, с перенацеливанием ссылок | `clone_resource`ᴱ | «Clone resources»; форма Clone resources: несколько ресурсов, целевой поток (в том числе новый), перенацеливание; Content Browser → контекстное меню ресурса → Clone, Clone to another stream…. Без рук: `ZONE=clone:stream=,index=1+2[,to=][,retarget=][,keep]` | `clone-resource` | — |
| Создать новый поток (.str) с записями в таблицах содержимого | `add_stream`ᴱ | «New stream», «New zone»; форма New stream: имя, родитель, способ загрузки; мастер New zone — поток, SimGroup и ячейка сетки одной операцией. Без рук: `ZONE=stream:name=,parent=[,load=on_demand\|with_parent\|auto_load]; WIZARD=zone,name=,parent=,min=,max=,step=3` | `add-stream` | — |
| Удалить поток, созданный рабочим пространством, со всем добавленным в него | `remove_stream`ᴱ | Tools → New stream… → список новых потоков → Remove; Content Browser → контекстное меню нового потока → Remove the new stream. Без рук: `ZONE=remove-stream:name=` | `remove-stream` | — |
| Переписать поток или ресурс прямо в папке игры, минуя рабочее пространство | **намеренно нет**: запись мимо рабочего пространства: без истории, отмены и diff. Сервер и окно пишут в игру только через apply | **намеренно нет**: то же: окно пишет в игру только через рабочее пространство | `repack`, `rewrite`, `replace` | — |
| Править ресурс во внешней программе: каждое сохранение импортируется | **намеренно нет**: следить за сохранениями чужой программы агенту незачем: он сам делает export → правка файла → import_file | полоса Open in external editor над просмотрщиком; вкладка Project → Open in external editor. Без рук: `EXTERNAL=<формат>` | `edit` | — |

### Модели, текстуры, материалы, коллизия

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Заменить меши модели игры файлом модели (glTF, OBJ, FBX, USD) | `import_model`ᴱ | «Replace a model»; Details ресурса mesh или metamodel → Import model…. Без рук: `WIZARD=model,file=,collision=; IMPORT=<файл> только открывает окно с планом` | `model-import` | `Workspace.import_model` |
| Импортировать модель как новый набор (metamodel, меши, материалы, текстуры) в поток образца | `import_model`ᴱ; name | «Place an object in a level»; окно Import model → New model set. Без рук: `WIZARD=place,file=,collision=` | `model-import`; --new | `Workspace.import_model` |
| Импортировать новую модель в другой поток (например, новый) с коллизией из геометрии | `import_model`ᴱ; stream, collision | окно Import model → New model set → Into stream (поток игры или новый) и Collision (mesh, hull, box). Без рук: `ZONE=import:file=,template=<поток>#<ресурс>,name=,stream=,collision=` | `model-import`; --new ИМЯ --to ПОТОК | — |
| Что импортёр читает из файла модели и план импорта без записи | `import_model`ᴱ; dry_run | окно Import model показывает план до записи. Без рук: `IMPORT=<файл>` | `model-info` | `read_model` |
| Построить коллизию Havok для metamodel: mesh, hull или box | `collision_build`ᴱ | меню Asset у metamodel и у её коллизии hkx (вкладка, Details) → Build collision → mesh, hull или box из собственного меша metamodel; переключатель коллизии в мастерах Replace a model и Place an object; флажок Own collision у клона в Add object. Без рук: `RESOURCE=[source=<файл модели>,]collision=mesh\|hull\|box` | `collision-build` | — |
| Посмотреть модель, коллизию, скелет в 3D | `resource_json`, `export`; смысловой эквивалент просмотра: данные и файл модели | вкладка ресурса: вид модели, панель материалов, вид коллизии. Без рук: `MODEL=…` | — | — |
| Заменить пиксели текстуры файлом PNG или DDS | `import_texture`ᴱ | просмотрщик текстуры → Import… | `texture-import` | `Workspace.import_texture`, `Texture.replaced_by` |
| Перекрасить модель: заменить текстуру во всех потоках, где лежат её копии | `import_texture`ᴱ; параметр all_copies: все копии текстуры одной операцией | «Retexture a model». Без рук: `WIZARD=retexture,set=,image=,only=N (текстура — только номером строки)` | `texture-import`; флаг --all-copies | — |
| Изменить параметры материала: оттенки g_TintColor и другие | `set_material`ᴱ, `import_model`ᴱ; set_material — материал игры или добавленный рабочим пространством; material_params у import_model — оттенки новых материалов импорта | редактор ресурса material (вкладка и Details): числа и цвета полями, текстуры хешем, Apply — одна операция; «Edit material…» в панели материала просмотрщика модели и персонажа; оттенки нового материала — в мастере New outfit. Без рук: `RESOURCE=param=<имя>=<n>/<n>/…,param=<имя>=tex:<хеш>; WIZARD=outfit,…,tint=<0..2>:<r>/<g>/<b>` | `material` | — |
| Интерфейс Flash (apx): заменить вложенную текстуру, ролик .apt, таблицу .const | `export`, `apx_edit`ᴱ; export пишет ролик .apt, таблицу .const и вложенные текстуры; apx_edit кладёт изменённую часть обратно | просмотрщик apx: Export, Import | `apx-export`, `apx-import` | — |

### Уровень и зоны

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Зоны уровня: список, что покажет 3D-вид | `level_objects`, `list_streams` | «Level editor», «New level tab», «Zones panel». Без рук: `LEVEL=<зона>[+<зона>]` | `level list`, `level info` | — |
| Пресеты зон по мировой сетке (уровень в ближней детализации, район, задания, контрольные точки) и сохранённые наборы зон | **намеренно нет**: пресет и набор выбирают, что показывает окно уровня; инструменты сервера принимают зоны и каталоги зон, а состав пресета даёт команда level presets | «Zones panel»; панель Zones → Presets и Saved sets; наборы — в project.toml открытого проекта, иначе в настройках игры; вкладка уровня открывается с загруженным в прошлый раз. Без рук: `LEVEL=preset:<id>[+set:<имя>…] \| LEVEL=last` | `level presets` — **частично**: список пресетов и слово preset:<id> в level info и level objects; сохранённые наборы — раздел [[zones]] в project.toml, отдельной команды нет | — |
| Список размещённых объектов: класс, имя, GUID, позиция, модель | `level_objects` | «Outliner panel». Без рук: `LEVEL=…;select=G:O` | `level objects` | — |
| 3D-вид уровня: камера, слои показа, выбор объекта щелчком и рамкой | `level_objects`, `editor_show_object`; смысловой эквивалент: объекты у точки (near, radius) и показ объекта в открытом окне — **частично**: картинки вида нет: агент не видит, как зона выглядит до запуска игры | вкладка уровня; панель Zones → Show; меню Level. Без рук: `LEVEL=…;eye=;look=;yaw=;pitch=;fov=;top;grid;stats;collision;…` | — | — |
| Переместить, повернуть, масштабировать объект уровня | `move_object`ᴱ | «Select», «Move», «Rotate», «Scale», «Local axes», «Snap always»; гизмо в виде; Details → Transform → Location, Rotation, Scale. Без рук: `LEVEL=…;select=G:O;act=move:dx,dy,dz \| act=turn:град (нет масштаба и абсолютной позиции)` | `simgroup-set` | — |
| Дублировать объект в его SimGroup | `duplicate_object`ᴱ | Details → Duplicate; Ctrl+D; Alt+перетаскивание гизмо. Без рук: `LEVEL=…;act=duplicate` | `level duplicate` | — |
| Удалить объект из SimGroup | `remove_object`ᴱ | Details → Remove; клавиша Delete. Без рук: `LEVEL=…;act=remove` | `level remove` | — |
| Добавить объект по образцу: с моделью образца, другой моделью или её клоном со своей коллизией | `add_object`ᴱ | «Place an object in a level»; Details → Add object (Metamodel id, Clone it, Own collision, Add to — SimGroup другой загруженной зоны, в том числе новой); перетаскивание модели из Content Browser в вид. Без рук: `LEVEL=…;act=add:<id>[:clone][:collision][:into=<зона>][:at=X/Y/Z]; WIZARD=place,model=,zones=` | `level add` | — |
| Поставить модель из файла на уровень одним действием: новый набор в потоке зоны, своя коллизия, объект на земле | `import_model`ᴱ, `add_object`ᴱ; двумя вызовами: import_model с name, stream и collision, затем add_object с id новой модели — **частично**: образец и поток называет агент, импорт и объект — две операции | «Place a model from a file»; файл модели, брошенный из файлового менеджера в вид уровня; меню Level и Add → Place a model from a file…; окно: имя, образец (Like), коллизия, размер текстур, план и просмотр. Без рук: `LEVEL=…;act=file:<файл>[:at=X/Y/Z][:name=][:collision=mesh\|hull\|box\|none][:limit=<px>]` | `model-import`, `level add` — **частично**: двумя командами: model-import --new --to --collision, затем level add --model; образец и поток называются руками | — |
| Добавить объект класса с пустыми атрибутами (без образца) | `add_object`ᴱ; параметр class вместо template | Details → Add object → Empty object of class (в SimGroup объекта или в выбранную в «Add to»). Без рук: `LEVEL=…;act=empty:<класс>[:into=<зона>]` | `simgroup-add` | — |
| Изменить атрибуты или модель объекта SimGroup | `set_attribute`ᴱ | Details → Advanced → Attributes; Details → Model → Choose another model; просмотрщик SimGroup | `simgroup-set` | — |
| Перенести, дублировать, удалить несколько объектов одной отменяемой операцией | `move_object`ᴱ, `duplicate_object`ᴱ, `remove_object`ᴱ; параметр objects: список объектов, одна операция | множественное выделение в Outliner или рамкой в виде, затем те же действия | — | — |
| Опустить объект на землю под ним | `move_object`ᴱ, `add_object`ᴱ, `ground`; параметр on_ground; ground — земля под точкой или точка на земле по лучу (то, что делает указатель) | клавиша End в виде уровня; перетаскивание модели ставит её на землю под указателем | — | — |
| Добавить в поток новую SimGroup: пустую или с копиями объектов другой | `add_simgroup`ᴱ | «New SimGroup», «New zone»; форма New SimGroup: пустая или копия объектов другой; мастер New zone; Content Browser → контекстное меню потока. Без рук: `ZONE=simgroup:stream=[,copy=<поток>#<ресурс>][,objects=0+1]` | `add-simgroup` | — |
| Добавить поток в мировую сетку уровня (wgi): игра грузит его у игрока | `add_zone_cell`ᴱ | «World grid cell», «New zone»; форма World grid cell: уровень, поток, коробка, сетка, имя ZoneStreamEnable; мастер New zone, шаг World grid. Без рук: `ZONE=cell:level=,stream=,min=X/Y/Z,max=X/Y/Z[,lod=][,name=][,on_enable]` | `world-grid add` | — |
| Мировая сетка уровня: показать ячейки, убрать ячейку потока | `zone_cells`, `remove_zone_cell`ᴱ | «World grid cell»; форма World grid cell → List the cells: ячейки уровня с кнопкой Remove у каждой. Без рук: `ZONE=remove-cell:level=,stream= (список — только в окне)` | `world-grid info`, `world-grid remove` | — |
| Править навигационный граф (пути ИИ): узлы, связи, флаги, R-дерево | `graph_edit`ᴱ | панель Zones → Navigation graphs: Pick nodes, Add node in front, Connect selected object, панель узла (тип графа, Rebuild R-tree); перетаскивание узла. Без рук: `LEVEL=…;nav=G:N;act=nav-rtree \| act=nav-flags:<hex> (узлы и связи — только мышью и полями)` | `graph-edit`, `graph-info`, `graph-export`, `graph-import` | `navigation_graph`, `navigation_graph_bytes` |
| Пройти по уровню и проверить маршрут из A в B по коллизии | `walk_route`; смысловой эквивалент: проверка прямого маршрута из A в B той же физикой, что у панели Walk; свободной ходьбы с клавиатуры нет | панель Zones → Walk: Walk from the camera, A here, B here, Walk from A to B, список Problems. Без рук: `LEVEL=…;walk;route=ax,ay,az,bx,by,bz` | — | — |
| Создать точку появления в месте камеры и запустить игру в ней | `play`ᴳ; параметр position (нужен --allow-edits) | панель Zones → Play from here → Make a point here and play; контекстное меню вида | — | — |

### Логика

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Граф логики: объекты, пины, сообщения, кто шлёт и кто слышит; миссии | `logic_graph` | «Logic graph»; Details объекта → Logic graph. Без рук: `LOGIC=<область>;find;show` | `logic-graph`, `messages`, `logic-missions` | `logic_graph`, `logic_messages`, `logic_missions` |
| Соединить пин отправителя с пином слушателя или разорвать связь | `logic_connect`ᴱ | граф логики: провод перетаскиванием между пинами; узел из образца правой кнопкой, Delete, Ctrl+D — **только мышью** | `logic-connect` | — |
| Схема пинов SimGroup: какие атрибуты — сообщения и ссылки | ресурс doctor-toolkit://schema/logic_schema.toml | пины узлов графа логики и подсказки значений при правке атрибутов | `logic-schema` | `logic_schema` |
| Прогнать логику миссии без игры: сообщения, переменные, таймеры, Lua | `logic_run`; журнал страницами; прогон повторяем, следующая страница — тот же вызов с offset | вкладка логики → Run mode: шаг, до времени, входы, точки останова, журнал. Без рук: `LOGIC=…;run=<события>;until=<с>` | `logic-run` | `logic_run` |

### Скрипты: Lua и визуальные

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Прочитать скрипт Lua игры: декомпиляция, листинг байткода | `resource_json`, `export` | просмотрщик Lua. Без рук: `LUA=…` | `lua-decompile`, `lua-export`, `lua-list` | `lua_decompile`, `lua_listing` |
| Изменить скрипт Lua игры: исходник компилируется в байткод игры | `import_file`ᴱ | просмотрщик Lua: редактор текста, Save, Load… | `lua-import`, `lua-compile` | `lua_compile` |
| Визуальный скрипт: создать, изменить, поставить в уровень, убрать | `edit_script`ᴱ | «Script editor»; редактор скриптов: палитра узлов, провода, Deploy, Remove. Без рук: `SCRIPT=<graph.json> открывает граф, но не ставит` | `script-deploy`, `script-remove`, `script-compile` | `script_compile` |
| Визуальные скрипты уровня и виды их узлов | `scripts`ᴱ; параметр node_kinds; формат графа — ресурс doctor-toolkit://docs/scripts.md | «Script editor»; File → Open, палитра узлов | `script-list`, `script-nodes` | — |
| Прогнать визуальный скрипт или файл Lua на макете игры | `script_run` | редактор скриптов → пробный прогон | `script-run` | — |

### Тексты, диалоги, субтитры

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Найти локализованную строку по тексту или хешу во всех таблицах и языках | `strings` | «Search the game»; запрос text:; просмотрщик таблицы строк с фильтром | `search`, `text-export` | `Workspace.string_table`, `StringTable.items`, `StringTable.get` |
| Изменить текст строки | `edit_string`ᴱ | «Edit texts»; просмотрщик таблицы строк: ячейка, Save; флажок «In all tables and languages» у Save и в мастере — запись во все таблицы и языки одной операцией; в мастере — поиск и замена, CSV. Без рук: `WIZARD=texts,table=,find=,replace=,replace_all[,everywhere]; RESOURCE=[language=<код>,]string=<имя или 0xхеш>:<текст> (запятая в значении — \, или значение строкой JSON)` | `text-import` — **частично**: только через JSON таблицы: команды для одной записи нет | `Workspace.edit_strings`, `Workspace.set_string_table`, `StringTable.set` |
| Добавить новую строку в таблицы языков с переводами | `add_string`ᴱ | просмотрщик таблицы строк → Add string (имя, текст, every language, «+ text of one language» — текст отдельного языка по коду); Add string в редакторе диалога. Без рук: `RESOURCE=add_string=<имя>:<текст>[,tr=<код языка>:<текст>…]` | `text-add` | — |
| Ссылки $name ролика интерфейса на строки: показать, перенаправить | `resource_json`, `apx_edit`ᴱ; ссылки $name — в resource_json ролика; перенаправление — text_refs у apx_edit | просмотрщик apx → Text references $name: список ссылок с хешем и числом, поле другой строки, Point. Без рук: `RESOURCE=apx=<старое имя>:<новое имя>` | `apx-text-refs` | — |
| Диалоги: список, поиск, дерево реплик с речью и текстами | `resource_json`, `search` | вкладка диалога (dlg). Без рук: `DIALOGUE=…` | `dialogues`, `dialogue`, `dialogue-export` | — |
| Изменить дерево диалога: реплики, варианты, флаги, тексты | `import_file`ᴱ; JSON диалога целиком | вкладка диалога: говорящий, текст, речь, варианты, вставить и убрать узел, Save | `dialogue-import` | — |
| Изменить текст субтитров речевого файла архива по языкам | `edit_subtitle`ᴱ | «Sound: archives, speech, mixer»; окно Sound → Archives → звук → субтитры по языкам → Apply subtitles; вкладка диалога → субтитр реплики. Без рук: `SOUND=select=<файл>,subtitle=<язык>:<текст>,apply` | `subtitle` | — |
| Изменить начало и длину субтитра | `edit_subtitle`ᴱ; параметры line, start_ms, duration_ms; строки и дорожки показывает cutscenes | вкладка ролика → дорожка Subtitles: Start, Length, Apply; перетаскивание по шкале времени. Без рук: `CUTSCENE=select=sub:N только выбирает` | `cutscene subtitle` | — |

### Звук

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Заменить звук банка sbk файлом WAV | `import_sound`ᴱ | вкладка банка sbk → Samples → Import… у строки | `sound-import`, `sound-encode` | `Workspace.import_sound`, `Sound.to_snu`, `sound_from_snu` |
| Заменить потоковый звук или речь архива .viv | `import_sound`ᴱ | «Sound: archives, speech, mixer»; окно Sound → Archives → звук → Replace…; вкладка диалога → Replace audio… у реплики. Без рук: `SOUND=select=<файл>,replace=<звуковой файл>` | `sound-import` | `Workspace.import_sound`, `Workspace.set_file` |
| Потоковые звуки и речь архивов .viv: список, сведения, экспорт в WAV | `media` | «Sound: archives, speech, mixer»; окно Sound → Archives: список с фильтром, сведения, Play, Export WAV…; Export WAV у реплики во вкладке диалога. Без рук: `SOUND=filter=<текст>,select=<файл>,export=<file.wav>` | `viv-list`, `audio` | `Game.archive_entries`, `Game.archive_sound` |
| Банк звуков: параметры звука, новые вариации и эмиттеры | `resource_json`, `sbk_edit`ᴱ, `sound_add`ᴱ | вкладка банка sbk → Structure: громкость, высота, спад, приоритет, Add sound…, New emitter, Save | `sbk-info`, `sbk-edit`, `sound-add` | `sound_bank` |
| Новая реплика из записи: звук, запись речевого банка, строка набора говорящего, событие | `voice_add`ᴱ, `speech`; speech с show: speakers перечисляет говорящих | «New voice line». Без рук: `WIZARD=voice,chv=,file=,event=,name=,subtitle=,only` | `voice-add` | — |
| Речевые банки и таблица событий речи: показать, добавить запись, изменить паузу и приоритет | `speech`, `set_speech_events`ᴱ | «Sound: archives, speech, mixer»; окно Sound → Speech banks (записи банков, фильтр) и Speech events (Set / add the record: пауза, приоритет, флаги, ранг); вид ресурсов smb и get с кнопкой Open in the Sound window. Без рук: `SOUND=page=banks,filter=<текст>; SOUND=event=<ИМЯ>,cooldown=,priority=,flags=,rank=,apply` | `speech-bank`, `speech-events` | — |
| Аудиоконфигурация и снимки микшера: категории, громкость, фильтр | `mix`, `set_mix`ᴱ | «Sound: archives, speech, mixer»; окно Sound → Audio configuration (категории, пулы, приглушения; новая категория) и Mixes (громкость, время, частота канала, новый канал); вид ресурсов auc и mix с кнопкой Open in the Sound window. Без рук: `SOUND=category=<ИМЯ>,parent=<ИМЯ>,apply; SOUND=mix=<имя>,channel=<КАТЕГОРИЯ>,volume=,time=,frequency=,apply` | `audio-config`, `mix` | — |

### Ролики и видео

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Ролики на движке: список кадров, шкала времени, актёры, звук, субтитры; предпросмотр | `cutscenes`, `resource_json`, `search` — **частично**: список роликов, кадр, шкала времени, актёры, звук и субтитры есть; предпросмотра (картинки кадра) нет | «Timeline panel», «Cut-scene Preview panel», «Cut-scene: play or pause»; вкладка ролика; Open in the level view. Без рук: `CUTSCENE=time,play,preview,…; CINE=time=,characters,through,view=` | `cutscene list`, `cutscene show` | — |
| Камера кадра: позиция, поворот, фокусное расстояние, ключи | `cutscene_camera`ᴱ, `cutscenes` | «Cut-scene: camera key from the view»; Details → Transform и Lens полями; Delete key, < Key, Key >; гизмо на камере | `cutscene export`, `cutscene import`, `cutscene camera` | — |
| Актёр в кадре: позиция, поворот, клип анимации | `cutscene_actor`ᴱ | Details выбранного актёра полями; гизмо на актёре | `cutscene clip`, `cutscene export`, `cutscene import`, `cutscene actor` | — |
| Команды кадра на шкале времени: текст на экране, время суток, сообщение, тени, масштаб времени | `cutscene_command`ᴱ, `import_file`ᴱ | вкладка ролика → инспектор команды: Kind, Text, Seconds, ключи кривой, Apply; сдвиг перетаскиванием. Без рук: `CUTSCENE=select=… только выбирает` | `cutscene move`, `cutscene export`, `cutscene import`, `cutscene command` | — |
| Вывести ролик: видео через камеры кадров, glTF с камерой и актёрами | `export` — **частично**: glTF кадра с камерой и актёрами есть (format glb); вывода в видео нет | Details → Export video: Size, Frames/s, Save as… | `cutscene render`, `cutscene gltf` | — |
| Заменить ролик .vp6: картинка и/или звуковая дорожка | `import_video`ᴱ | «Movies»; вкладка Movies → Import video…, Replace audio only…. Без рук: `MOVIES=<ролик> только открывает` | `video-import` | — |
| Ролики .vp6: список, сведения, кадры в PNG, звук в WAV | `media` | «Movies»; проигрыватель, Export audio (WAV)…, Export frame (PNG)… | `video-info`, `video-export` | — |

### Персонажи, анимации, параметры игры

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| База частей персонажей: группы, наборы, слоты, варианты, части | `resource_json` | вкладка partsdb: набор, варианты. Без рук: `CHARACTER=set,lod,choose,select` | `partsdb list`, `partsdb show` | — |
| Собрать персонажа из набора и вывести одной моделью glTF | `export`; формат glb для partsdb с параметрами set, choices, lod | вкладка partsdb: Random, Export | `partsdb character` | `character_glb` |
| Новая часть одежды: модель, материалы с оттенками, часть и вариант, разрешить в наборах | `import_model`ᴱ, `partsdb_add_part`ᴱ, `partsdb_allow`ᴱ, `set_material`ᴱ; import_model с name делает меш и материалы (оттенки — material_params), partsdb_add_part — часть, вариант и наборы | «New outfit». Без рук: `WIZARD=outfit,slot=,set=,template=<часть-образец>,file=,lod=<файл второго уровня детализации>,name=,tint=<0..2>:<r>/<g>/<b>,tags=,only` | `partsdb add-part`, `partsdb allow` | — |
| Банки анимаций: клипы, события, граф узлов; экспорт клипов | `resource_json`, `export` | вкладка банка bnk: клипы, Graph, Export. Без рук: `ANIM=clip,frame,graph,node` | `anim list`, `anim export`, `anim node-types` | `animation_bank`, `animation_graph`, `animation_glb` |
| Импортировать клип анимации (заменить или добавить), изменить события клипа и граф узлов | `import_animation`ᴱ, `clip_events`ᴱ, `animation_graph`, `set_animation_graph`ᴱ | вкладка банка bnk: Import clip…, Replace clip, Add clip, петля; Graph: узлы, параметры, Apply. Без рук: `ANIM=import=<файл>,add=<имя>` | `anim import`, `anim events`, `anim graph` | `Workspace.import_animation`, `Workspace.set_clip_events`, `Workspace.set_animation_graph` |
| Изменить параметры игры из курируемого списка во всех копиях атрибута | `tunables`, `set_tunables`ᴱ | «Change game parameters». Без рук: `WIZARD=params,add=,category=,value=,factor=,find=` | `tunables` | — |

### Рабочее пространство и проект

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Состояние рабочего пространства: изменения, новые потоки, история операций | `workspace_status` | «Changes panel», «Undo history panel». Без рук: `CHANGES=1; SHELL=drawer=history` | `workspace status`, `workspace log`, `workspace init` | `Game.workspace`, `Game.init_workspace`, `Workspace.status`, `Workspace.changes`, `Workspace.history` |
| Что каждое изменение делает против оригинала | `workspace_diff` | «Changes panel»; панель Changes: строка изменения → диапазоны и строки | `workspace diff` | `Workspace.diff` |
| Отменить и вернуть операцию | `undo`ᴱ, `redo`ᴱ | «Undo», «Redo»; панель History: щелчок по операции | `workspace undo`, `workspace redo` | `Workspace.undo`, `Workspace.redo` |
| Убрать одно изменение или все из рабочего пространства | `revert`ᴱ | панель Changes → Revert this change, Discard all changes | `workspace revert` | `Workspace.revert`, `Workspace.revert_files`, `Workspace.revert_all` |
| Сохранить несохранённые правки открытых просмотрщиков в рабочее пространство | **намеренно нет**: у инструмента нет несохранённого состояния: каждый вызов сразу записан операцией | «Save» | — | — |
| Записать рабочее пространство в файлы игры | `apply`ᴳ | «Apply changes to game»; панель Changes → Apply to game | `workspace apply` | `Workspace.apply` |
| Вернуть файлам игры исходный вид, оставив правки в рабочем пространстве | `unapply`ᴳ | «Revert game files»; панель Changes → Revert game | `workspace revert`; флаг --game | `Workspace.unapply` |
| Восстановить оригиналы всех файлов, которые менял тулкит | `restore_originals`ᴳ; под --allow-game; отказ при запущенной игре | «Restore originals» | `restore` | — |
| Проект мода: состояние исходных файлов и сборка изменённых в рабочее пространство | `project_status`, `project_build`ᴱ | «Project browser»; вкладка Project → Build, Refresh. Без рук: `PROJECT=<папка>` | `project status`, `project build` | `Project.status`, `Project.build`, `Project.entries` |
| Создать проект из потоков игры или изменений (с добавленными ресурсами и новыми потоками); записать версию рабочего пространства в файлы проекта; записать исходники всего, что держит рабочее пространство проекта | `project_new`, `project_pull`ᴱ; project_new пишет в папку вывода сервера (--output-dir); project_pull — в проект сервера (--project), с параметром sources — исходники всего рабочего пространства проекта | «Project browser»; вкладка Project → Create, Open, Pull (версия рабочего пространства — в файлы проекта), Write sources (исходники всего рабочего пространства проекта). Без рук: `PROJECT=<папка> и SHELL=pull, SHELL=sources` | `project new`, `project pull`, `project sources` | `Project.create`, `Project.pull`, `Project.sources` |
| Проект как документ: папка со своим рабочим пространством — создать пустой, открыть, закрыть | `project_new`, `project_status`; project_new без streams и from_workspace — пустой проект со своим рабочим пространством; сервер с --project работает с рабочим пространством проекта | «New project», «Open project», «Close project»; меню File: New project…, Open project…, Recent projects, Close project; недавние проекты на Welcome; имя проекта в заголовке окна и строке состояния. Без рук: `OPEN_PROJECT=<папка>` | `project new`, `project status`, `project build`; project new GAME DIR без --stream и --from-workspace — пустой проект; команды project … и ключ --workspace-dir <проект>/workspace работают с его рабочим пространством | `Project.create`, `Project.status`, `Project.build`; Project.create(game, path, empty=True); Project(path) работает с рабочим пространством проекта |
| Сделать проектом рабочее пространство игры: правки и история переезжают в собственное рабочее пространство нового проекта | **намеренно нет**: сервер работает с рабочим пространством, с которым запущен: перенос из-под работающего сервера оставил бы его без правок. Перенос — командой project new --move-workspace или в редакторе, после чего сервер запускают с --project | «Make a project of the workspace»; меню File: Make a project of the workspace… | `project new`; project new GAME DIR --move-workspace [--no-sources]: перенос и исходники перенесённого | — |
| База проекта — моды, на которых он сделан: записать из игры, сверить с игрой, привести моды к базе | `project_status`; расхождения базы с модами игры — в warnings — **частично**: записать базу и привести к ней моды инструментом нельзя | «Take the base from the mods in the game», «Bring the mods to the project's base»; меню File: Take the base from the mods in the game, Bring the mods to the project's base; полоса вверху окна — расхождения с кнопками и предложение записать базу; сверка при открытии проекта, Play и Install project into Mods | `project base`; без ключей — база и расхождения; --record, --check (ошибка при расхождении), --apply | — |
| Перебазирование: найти правки, сделанные на прежних данных модов под ними, и перенести их на то, что игра даёт сейчас, — слить по операциям, оставить своё или взять данные игры | `rebase`ᴱ; check — только список; only, keep, take_base; устаревшие правки проекта — в warnings инструмента project_status | «Rebase the edits onto what the game gives now»; меню File: Rebase the edits onto what the game gives now; полоса вверху окна проекта — «N edits were made on other data of the base» с кнопками Rebase и Changes; панель Changes — пометка stale, общий Rebase и для выбранной правки Rebase, Keep mine, Take the base's со сравнением «что игра изменила под правкой»; проверка при открытии рабочего пространства, после смены модов, перед Play и при Install project into Mods | `project rebase`, `workspace rebase`; без ключей — перенести все устаревшие правки одной операцией; --check (только список, ошибка при устаревших), --only KEY, --keep KEY, --take-base KEY; предупреждение в project status, project mod, project install, mods build | `Workspace.stale`, `Workspace.rebase` |

### Моды

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Собрать файл мода из изменений: имя, id, версия, автор, связи, издания, выбор изменений | `mod_build`ᴱ | «Package a mod»; окно Mods → раздел сборки; вкладка Project → Build mod. Без рук: `WIZARD=package,name=,author=,description=,out= (нет id, версии, издания, связей)` | `mods build`, `project mod` | `Workspace.build_mod`, `Project.build_mod` |
| Собрать мод с исполняемым кодом (формат 5): модули .asi, «код обязателен», файлы классов | `mod_build`ᴱ; параметры native, native_required, native_plain | окно Mods → раздел сборки: Native modules (строки имя=путь), code required; мастер Package a mod → связи | `mods build`, `class-schema`; --native, --native-required | — |
| Собрать мод с дельтами (формат 6): изменённые зоны (SimGroup), таблицы строк, базы частей и мировые сетки хранятся как операции, которые менеджер объединяет с другими модами; отказ от дельт — целые ресурсы для прежних менеджеров | `mod_build`ᴱ; параметр deltas (false — целые ресурсы); ответ называет число дельт; у проекта — [build] deltas в project.toml | окно Mods → раздел сборки и мастер Package a mod → Dependencies, editions and load order: флажок Keep whole resources (no merging); проверка мастера называет число дельт | `mods build`, `mods install-workspace`; --no-deltas; у проекта (project mod, project install) — [build] deltas = false в project.toml | `Workspace.build_mod`, `ModManager.install_workspace` |
| Менеджер модов: установленные моды, состояние в игре, конфликты (целых ресурсов и по элементам у модов с дельтами), совместимость | `mods` | «Mods»; окно Mods: карточки, Details. Без рук: `MODS=1` | `mods list`, `mods status`, `mods conflicts`, `mods check` | `Game.mods`, `ModManager.mods`, `ModManager.status`, `ModManager.conflicts`, `ModManager.clashes`, `ModManager.check` |
| Что в файле мода (.zip, .vpat) до установки (ресурсы, дельты и число операций в них, потоки, связи, код), и его проверка | `mods`; действие info (на сервере с --allow-edits или --allow-game: читает файл по пути) | окно Mods → Install…: вопрос перед установкой показывает, что в каждом файле (ресурсы, потоки, связи, код), и проверяет его. Без рук: `MODS=info=<файл>` | `mods info` | `mod_info` |
| Установить, удалить, включить, выключить мод; порядок загрузки и сортировка по правилам | `mods_manage`ᴳ | «Mods»; окно Mods: Install…, перетаскивание файла и карточек, переключатель, контекстное меню, Sort | `mods install`, `mods uninstall`, `mods enable`, `mods disable`, `mods move`, `mods sort` | `ModManager.install`, `ModManager.uninstall`, `ModManager.enable`, `ModManager.move`, `ModManager.sort` |
| Записать включённые моды в игру и снять их | `mods_manage`ᴳ | «Mods»; окно Mods → Apply, Unapply. Без рук: `DOCTOR_TOOLKIT_MODS_ASK=write открывает вопрос` | `mods apply`, `mods unapply` | `ModManager.apply`, `ModManager.unapply` |
| Поставить изменения рабочего пространства как мод | `mods_manage`ᴳ; действие install_workspace | «Mods», «Install project into Mods»; окно Mods → раздел сборки → Install as mod; открытый проект — File → Install project into Mods (мод с id, именем и версией проекта) | `mods install-workspace`, `project install`; project install — проект как мод с его id, именем и версией | `ModManager.install_workspace` |
| Перенять игру, пропатченную старым тулкитом, и её патчи как моды | `mods_manage`ᴳ; действие import_old | «Mods»; окно Mods → More → импорт старых модов; вопрос первого запуска | `mods import-old` | — |
| Записать установленный мод файлом мода | `mods_manage`ᴳ; действие export (в папку --output-dir) | окно Mods → меню карточки мода → Save as file…. Без рук: `MODS=export=<id>:<файл>` | `mods export` | `ModManager.export` |
| Наборы модов: сохранить, выбрать, удалить | `mods_manage`ᴳ, `mods`; действия save_set, use_set, delete_set; mods: sets | «Mods»; окно Mods: выбор набора, Save, удалить | — | — |
| Исполняемый код модов: состояние и что мод поставил бы в игру | `mods`; действие native; поле code у list | «Mods»; окно Mods: значок на карточке, состояние Waiting for your consent | `mods native` | — |
| Согласие игрока на исполняемый код мода: разрешить, только данные, забыть | **намеренно нет**: согласие даёт человек: агент не может разрешить запуск чужого кода на машине игрока | «Mods»; окно вопроса: Allow, Disable, Later, Forget — **только мышью** | `mods native`; --allow, --data-only, --forget | — |
| Согласие на модуль отчётов о падениях в игре: оставить или убрать | **намеренно нет**: согласие даёт человек: модуль остаётся в игре игрока и после работы агента | вопрос при первом запуске; флажок в Play options и в Mods → More — **только мышью** | `crash-report install`, `crash-report uninstall` | — |

### Запуск игры

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Запустить игру с правками (обычно или сразу в потоке уровня у точки появления) и остановить | `play`ᴳ, `stop`ᴳ | «Play», «Play options»; вкладка Game: Play, Stop, Restart; панель Zones → Play from here. Без рук: `SHELL=play только открывает окно параметров` | `play` | — |
| Параметры запуска: способ (Steam, Proton, своя команда), графический модуль, окно | `play`ᴳ; size, render, fps, fov, fxaa, sharpen, draw_merge, laa, dxvk, vulkan_driver | «Play options» | `play`, `render config`, `render install`, `render uninstall`, `render log` | — |
| Снимок кадра запущенной игры | `screenshot`ᴳ | вкладка Game → Screenshot (игра во вкладке) | — | — |
| Отчёты о падениях игры: список, показать, состояние модуля, адрес по базе знаний | `crash_reports`, `kb_search` | «Crash reports»; Tools → Crash reports: список отчётов (новые сверху), текст отчёта с именами сообщений, состояние модуля, папка; Play options → Last crash. Без рук: `SHELL=crashes[=<номер или имя отчёта>]` | `crash-report show`, `crash-report list`, `crash-report status`, `crash-report log`, `crash-report lookup` | — |

### Живая связь с игрой

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Связь с запущенной игрой: есть ли она, состояние цикла, загруженные потоки | `live_status` | вкладка Game → Link: Connect, Disconnect, Streams. Без рук: `LIVE=PORT[:TOKEN]` | `live status`, `live streams`, `live stream list` | — |
| Поставить модуль связи в папку игры и убрать его | `play`ᴳ, `stop`ᴳ; play ставит модуль сам, stop убирает | вкладка Game → Link: Live link: put the module into the game for Play | `live install`, `live uninstall` | — |
| Игрок и камера игры: позиция, направление, поле зрения | `player` | панель Link: Player, Camera | `live player`, `live camera` | — |
| Объекты запущенной игры: рядом с точкой, по GUID, по имени | `objects_near`, `live_object` | панель Link: Objects nearby, All objects → List, Find; Details → In the running game → Read | `live objects`, `live find`, `live object get` | — |
| Состояние игры: время суток, погода, деньги, команда, контрольная точка, переменные сюжета | `game_state`, `variables` | панель Link → World and state: Read | `live time`, `live weather`, `live money`, `live family`, `live mission`, `live var get` | — |
| Перенести игрока в точку | `teleport`ᴳ | панель Link → Teleport…; в виде уровня — Player here | `live teleport` | — |
| Отправить сообщение на шину SimGroup: всем слушателям или одному объекту | `send_message`ᴳ | панель Link → Message, to, Send… | `live send` | — |
| Выполнить Lua в запущенной игре | `lua`ᴳ | панель Link → Lua 5.0 in the game, Run… | `live lua` | — |
| Задать время суток и его скорость, деньги, переменную сюжета | `set_time`ᴳ, `set_money`ᴳ, `set_variable`ᴳ | панель Link → World and state: Set time, Set rate, Money Set / Add, Story variables Set | `live time`, `live money`, `live var set` | — |
| Загрузить и выгрузить поток в запущенной игре | `stream_load`ᴳ, `stream_unload`ᴳ | панель Link → World and state → Streams: Load, Unload | `live stream load`, `live stream unload` | — |
| Править мир на ходу: сдвинуть, повернуть, скрыть объект, включить и выключить его логику, отменить | `move_live_object`ᴳ, `live_object_enable`ᴳ, `live_object_undo`ᴳ, `live_object_edits` | Details → In the running game: Gizmo moves it in the game too, Place in the game, скрыть и показать, флажок Logic (логика объекта), Undo in the game; панель Link → Edits of the world. Без рук: `LIVE=PORT[:TOKEN] и LIVE_DO=get=<guid>,hide=<guid>,show=<guid>,logic=<guid>:on\|off,undo (без вопроса: переменная — согласие)` | `live object move`, `live object rotate`, `live object disable`, `live object enable`, `live object undo`, `live object edits` | — |
| Записать сдвинутые в игре объекты в рабочее пространство | `live_objects_to_workspace`ᴱ | Details → In the running game → Move into the workspace | `live object commit` | — |
| Измерить игру изнутри: время кадра, ЦП потоков, чтения файлов, текстуры, память | `live_profile` | вкладка Game → панель Link → Profile: Measure, Measure and reset, Memory, Sample; замер текстом, как у live profile. Без рук: `LIVE=PORT[:TOKEN] и LIVE_DO=profile[=reset+memory+sample:<мс>]` | `live profile` | — |
| Свободная камера: игра рисует из заданной точки; камера редактора ↔ камера игры | `camera`ᴳ, `player` | вид уровня: Follow the game's camera, Camera → game, Game's camera. Без рук: `LIVE_FOLLOW=1 (только следование)` | `live freecam` | — |
| Отладка вживую: пауза, шаг, точки останова на сообщении, печати и ошибке Lua; логика и скрипты вживую | `pause`ᴳ, `resume`ᴳ, `breakpoints`ᴳ | панель Link → Debug: Pause, Continue, Step, Add breakpoint; вкладка логики → Live; редактор скриптов: Stop the game at this node. Без рук: `LOGIC=…;live` | `live pause`, `live continue`, `live break` | — |
| Журнал игры: печать, ошибки Lua со стеком, сообщения шины | `game_log` | панель Link → Game journal; вкладка логики → Live, Save… | `live log` | — |
| Отправить изменённые текстуры рабочего пространства в запущенную игру без перезапуска | `push_textures`ᴳ | панель Link → Debug → Send edited textures | `live texture` | — |
| Диагностика связи: файлы и ресурсы, которые модуль отдаёт игре, сверка ресурсов в памяти, чтение памяти | **намеренно нет**: диагностика самого модуля связи для разработчиков тулкита; состояние игры агенту дают live_status и live_profile | **намеренно нет**: то же: диагностика модуля, не работа с игрой | `live files`, `live resources`, `live resource`, `live resource-check`, `live peek` | — |

### Проверки, сохранения, диагностика

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Проверить правки перед записью в игру: ресурсы читаются, модели подходят классам, ссылки сходятся, сообщения, конфликты | `validate`ᴱ | «Check edits»; панель Findings; та же проверка перед Apply to game (ошибки держат запись до «Apply anyway») и на шаге Check мастера Package a mod. Без рук: `SHELL=check (ошибки в правках — ненулевой код выхода)` | `check-edits`, `logic-check` | `logic_check` — **частично**: только проверка сообщений |
| Проверить ссылки потоков игры: всё названное лежит в потоке или в загруженном раньше | `check_refs` | Content Browser → меню потока или папки → Check references; находки — в панели Findings. Без рук: `SHELL=refs=<поток или папка>` | `check-refs` | — |
| Сверить файлы игры с оригиналами: кто что менял | `integrity` | «Check game files»; окно Mods → More → Check integrity | `integrity` | `Game.integrity` |
| Проверить, что каждый поток и таблица содержимого папки игры читаются | **намеренно нет**: проверка разбора контейнера — тест самого тулкита; целостность файлов игры даёт integrity | **намеренно нет**: то же; в окне целостность проверяет «Check game files» | `validate` | — |
| Заглянуть в файл сохранения: сводка, переменные, деньги, семьи, здания (только чтение) | `save_info` | «Look into a save file». Без рук: `SAVE=<файл>` | `save` | — |

### База знаний и справочные данные

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| База знаний: поиск по заметкам и чтение заметки | `kb_search`, `kb_note`; ресурсы doctor-toolkit://kb/… | «Knowledge base»; вкладка Knowledge base: поиск, заметка, переход по ссылкам; ссылка «Knowledge base note» в Details объекта (класс) и ресурса (тип). Без рук: `SHELL=kb=<слова через +> \| kb=<заметка.md>` | — | — |
| Типизированные данные игры: списки значений, сведения о сообщении, смысл атрибутов класса | `game_types` | вкладка Knowledge base → Data types: списки значений, сообщения по части имени, атрибуты класса; выбор значения из списка при правке атрибута. Без рук: `SHELL=types=<список> \| types=message:<текст> \| types=class:<имя>` | `game-types enums`, `game-types message`, `game-types attrs`, `game-types stats` | `enum_values`, `message_info`, `message_search`, `attr_types` |
| Документы тулкита: форматы файлов, руководство | ресурсы doctor-toolkit://docs/… | «User guide» | — | — |

### Окно редактора и служебное

| Возможность | MCP | Редактор | CLI | Python |
|---|---|---|---|---|
| Окно редактора: панели, вкладки, раскладки, темы, масштаб, настройки, справка по клавишам | **намеренно нет**: состояние окна человека — раскладка, тема, масштаб — к игре и правкам отношения не имеет | «Log panel», «Welcome screen», «Close tab», «Quit», «Preferences», «Dark theme», «Light theme», «Theme of the system», «Zoom in», «Zoom out», «Reset zoom», «Maximize tab», «Full screen», «Save layout as», «Reset layout», «Keyboard shortcuts», «Show log file», «About doctor_toolkit», «What do you want to do?». Без рук: `SHELL=prefs,about,shortcuts,welcome,log,layout=,drawer=,max,…; SHOT=<файл> — снимок окна и выход` | — | — |
| Мост агента к открытому окну: что выделил человек, показать ему ресурс или объект | `editor_selection`, `editor_open`, `editor_show_object` | **намеренно нет**: для человека это и есть выделение и переход в окне; редактор только отвечает на мост (и спрашивает, прежде чем одолжить рабочее пространство) | — | — |
| Запустить MCP-сервер для клиента ИИ | **намеренно нет**: сервер запускает клиент ИИ | **намеренно нет**: сервер запускает клиент ИИ, а не окно; окно отвечает ему через мост | `mcp` | — |
| Служебное для разработчиков тулкита: генерация и проверка базы знаний, типов данных и этой таблицы | **намеренно нет**: команды работают с рабочей копией репозитория тулкита, а не с игрой | **намеренно нет**: то же | `kb-gen`, `kb-lint`, `types-gen`, `parity-gen` | — |
| Служебные проверки: кодек анимаций туда-обратно, ответчик вместо игры, падение игры нарочно | **намеренно нет**: проверки самого тулкита для его разработчиков | **намеренно нет**: то же | `anim encode-test`, `live fake`, `crash-report test` | — |

## Есть в MCP, нет в редакторе

Нет.

### В редакторе есть только часть

Нет.

## Есть в редакторе, нет в MCP

Нет.

### В MCP есть только часть

- **3D-вид уровня: камера, слои показа, выбор объекта щелчком и рамкой** (`level.view`): вкладка уровня; панель Zones → Show; меню Level. В MCP не хватает: картинки вида нет: агент не видит, как зона выглядит до запуска игры.
- **Поставить модель из файла на уровень одним действием: новый набор в потоке зоны, своя коллизия, объект на земле** (`level.place-file`): «Place a model from a file»; файл модели, брошенный из файлового менеджера в вид уровня; меню Level и Add → Place a model from a file…; окно: имя, образец (Like), коллизия, размер текстур, план и просмотр. В MCP не хватает: образец и поток называет агент, импорт и объект — две операции.
- **Ролики на движке: список кадров, шкала времени, актёры, звук, субтитры; предпросмотр** (`cine.read`): «Timeline panel», «Cut-scene Preview panel», «Cut-scene: play or pause»; вкладка ролика; Open in the level view. В MCP не хватает: список роликов, кадр, шкала времени, актёры, звук и субтитры есть; предпросмотра (картинки кадра) нет.
- **Вывести ролик: видео через камеры кадров, glTF с камерой и актёрами** (`cine.export`): Details → Export video: Size, Frames/s, Save as…. В MCP не хватает: glTF кадра с камерой и актёрами есть (format glb); вывода в видео нет.
- **База проекта — моды, на которых он сделан: записать из игры, сверить с игрой, привести моды к базе** (`project.base`): «Take the base from the mods in the game», «Bring the mods to the project's base»; меню File: Take the base from the mods in the game, Bring the mods to the project's base; полоса вверху окна — расхождения с кнопками и предложение записать базу; сверка при открытии проекта, Play и Install project into Mods. В MCP не хватает: записать базу и привести к ней моды инструментом нельзя.

## Есть только в командной строке

Ни сервер MCP, ни редактор этого не дают; то, чего у них не будет, — в следующем разделе.

Нет.

## Намеренно нет

- **Список хешей игры без известного имени** (`game.hash-unknown`) — нет в MCP: исследовательская сводка для пополнения словаря имён тулкита, а не действие над игрой или модом.
- **Список хешей игры без известного имени** (`game.hash-unknown`) — нет в редакторе: то же: нужна разработчику тулкита, не автору мода.
- **Переписать поток или ресурс прямо в папке игры, минуя рабочее пространство** (`res.direct-write`) — нет в MCP: запись мимо рабочего пространства: без истории, отмены и diff. Сервер и окно пишут в игру только через apply.
- **Переписать поток или ресурс прямо в папке игры, минуя рабочее пространство** (`res.direct-write`) — нет в редакторе: то же: окно пишет в игру только через рабочее пространство.
- **Править ресурс во внешней программе: каждое сохранение импортируется** (`res.external`) — нет в MCP: следить за сохранениями чужой программы агенту незачем: он сам делает export → правка файла → import_file.
- **Пресеты зон по мировой сетке (уровень в ближней детализации, район, задания, контрольные точки) и сохранённые наборы зон** (`level.presets`) — нет в MCP: пресет и набор выбирают, что показывает окно уровня; инструменты сервера принимают зоны и каталоги зон, а состав пресета даёт команда level presets.
- **Сохранить несохранённые правки открытых просмотрщиков в рабочее пространство** (`ws.save`) — нет в MCP: у инструмента нет несохранённого состояния: каждый вызов сразу записан операцией.
- **Сделать проектом рабочее пространство игры: правки и история переезжают в собственное рабочее пространство нового проекта** (`project.adopt`) — нет в MCP: сервер работает с рабочим пространством, с которым запущен: перенос из-под работающего сервера оставил бы его без правок. Перенос — командой project new --move-workspace или в редакторе, после чего сервер запускают с --project.
- **Согласие игрока на исполняемый код мода: разрешить, только данные, забыть** (`mods.native-consent`) — нет в MCP: согласие даёт человек: агент не может разрешить запуск чужого кода на машине игрока.
- **Согласие на модуль отчётов о падениях в игре: оставить или убрать** (`crash.consent`) — нет в MCP: согласие даёт человек: модуль остаётся в игре игрока и после работы агента.
- **Диагностика связи: файлы и ресурсы, которые модуль отдаёт игре, сверка ресурсов в памяти, чтение памяти** (`live.internals`) — нет в MCP: диагностика самого модуля связи для разработчиков тулкита; состояние игры агенту дают live_status и live_profile.
- **Диагностика связи: файлы и ресурсы, которые модуль отдаёт игре, сверка ресурсов в памяти, чтение памяти** (`live.internals`) — нет в редакторе: то же: диагностика модуля, не работа с игрой.
- **Проверить, что каждый поток и таблица содержимого папки игры читаются** (`check.container`) — нет в MCP: проверка разбора контейнера — тест самого тулкита; целостность файлов игры даёт integrity.
- **Проверить, что каждый поток и таблица содержимого папки игры читаются** (`check.container`) — нет в редакторе: то же; в окне целостность проверяет «Check game files».
- **Окно редактора: панели, вкладки, раскладки, темы, масштаб, настройки, справка по клавишам** (`shell.window`) — нет в MCP: состояние окна человека — раскладка, тема, масштаб — к игре и правкам отношения не имеет.
- **Мост агента к открытому окну: что выделил человек, показать ему ресурс или объект** (`shell.bridge`) — нет в редакторе: для человека это и есть выделение и переход в окне; редактор только отвечает на мост (и спрашивает, прежде чем одолжить рабочее пространство).
- **Запустить MCP-сервер для клиента ИИ** (`shell.mcp-server`) — нет в MCP: сервер запускает клиент ИИ.
- **Запустить MCP-сервер для клиента ИИ** (`shell.mcp-server`) — нет в редакторе: сервер запускает клиент ИИ, а не окно; окно отвечает ему через мост.
- **Служебное для разработчиков тулкита: генерация и проверка базы знаний, типов данных и этой таблицы** (`shell.generated`) — нет в MCP: команды работают с рабочей копией репозитория тулкита, а не с игрой.
- **Служебное для разработчиков тулкита: генерация и проверка базы знаний, типов данных и этой таблицы** (`shell.generated`) — нет в редакторе: то же.
- **Служебные проверки: кодек анимаций туда-обратно, ответчик вместо игры, падение игры нарочно** (`shell.selftest`) — нет в MCP: проверки самого тулкита для его разработчиков.
- **Служебные проверки: кодек анимаций туда-обратно, ответчик вместо игры, падение игры нарочно** (`shell.selftest`) — нет в редакторе: то же.

## В редакторе только мышью

Эти действия нельзя задать ни полями, ни клавишей, ни предустановкой: агент, который ведёт редактор без рук, их не сделает и может не заметить.

- **Соединить пин отправителя с пином слушателя или разорвать связь** (`logic.connect`): граф логики: провод перетаскиванием между пинами; узел из образца правой кнопкой, Delete, Ctrl+D.
- **Согласие игрока на исполняемый код мода: разрешить, только данные, забыть** (`mods.native-consent`): «Mods»; окно вопроса: Allow, Disable, Later, Forget.
- **Согласие на модуль отчётов о падениях в игре: оставить или убрать** (`crash.consent`): вопрос при первом запуске; флажок в Play options и в Mods → More.

## Что редактор делает без рук

Предустановки `DOCTOR_TOOLKIT_GUI_…` ([оболочка](shell.md)) есть у 78 возможностей редактора из 141; остальным нужен человек. Правки записывают только `LEVEL …;act=` и `WIZARD=…,step=N`, прочие открывают и показывают.

- **Список ресурсов потока** (`game.stream-resources`): `SELECT=N выбирает ресурс по номеру (по id или имени — нет)`.
- **Поиск по всей игре: имя, шаблон, хеш, ключ, тип, поток, текст строк** (`game.search`): `SEARCH=<запрос>; SHELL=palette=<текст>; CONTENT=<место>`.
- **Построить или обновить индекс игры** (`game.index`): `SHELL=index`.
- **Имена хеша и хеши имени под каждой хеш-функцией движка** (`game.hash`): `SHELL=hash=<имя или хеш>`.
- **Вернуть отредактированный экспорт: JSON строк, логики, графа, диалога, исходник Lua, сырые данные** (`res.import-file`): `RESOURCE=import=<файл> | raw=<файл>`.
- **Добавить в поток новый ресурс из файла сырых данных** (`res.add-raw`): `ZONE=resource:stream=,type=,file=[,ids=][,policy=][,compress]`.
- **Клонировать ресурсы под новыми id: в тот же или другой поток, с перенацеливанием ссылок** (`res.clone`): `ZONE=clone:stream=,index=1+2[,to=][,retarget=][,keep]`.
- **Создать новый поток (.str) с записями в таблицах содержимого** (`res.stream-new`): `ZONE=stream:name=,parent=[,load=on_demand|with_parent|auto_load]; WIZARD=zone,name=,parent=,min=,max=,step=3`.
- **Удалить поток, созданный рабочим пространством, со всем добавленным в него** (`res.stream-remove`): `ZONE=remove-stream:name=`.
- **Править ресурс во внешней программе: каждое сохранение импортируется** (`res.external`): `EXTERNAL=<формат>`.
- **Заменить меши модели игры файлом модели (glTF, OBJ, FBX, USD)** (`model.replace`): `WIZARD=model,file=,collision=; IMPORT=<файл> только открывает окно с планом`.
- **Импортировать модель как новый набор (metamodel, меши, материалы, текстуры) в поток образца** (`model.new-set`): `WIZARD=place,file=,collision=`.
- **Импортировать новую модель в другой поток (например, новый) с коллизией из геометрии** (`model.new-set-stream`): `ZONE=import:file=,template=<поток>#<ресурс>,name=,stream=,collision=`.
- **Что импортёр читает из файла модели и план импорта без записи** (`model.file-info`): `IMPORT=<файл>`.
- **Построить коллизию Havok для metamodel: mesh, hull или box** (`model.collision`): `RESOURCE=[source=<файл модели>,]collision=mesh|hull|box`.
- **Посмотреть модель, коллизию, скелет в 3D** (`model.view`): `MODEL=…`.
- **Перекрасить модель: заменить текстуру во всех потоках, где лежат её копии** (`texture.retexture`): `WIZARD=retexture,set=,image=,only=N (текстура — только номером строки)`.
- **Изменить параметры материала: оттенки g_TintColor и другие** (`material.params`): `RESOURCE=param=<имя>=<n>/<n>/…,param=<имя>=tex:<хеш>; WIZARD=outfit,…,tint=<0..2>:<r>/<g>/<b>`.
- **Зоны уровня: список, что покажет 3D-вид** (`level.zones`): `LEVEL=<зона>[+<зона>]`.
- **Пресеты зон по мировой сетке (уровень в ближней детализации, район, задания, контрольные точки) и сохранённые наборы зон** (`level.presets`): `LEVEL=preset:<id>[+set:<имя>…] | LEVEL=last`.
- **Список размещённых объектов: класс, имя, GUID, позиция, модель** (`level.objects`): `LEVEL=…;select=G:O`.
- **3D-вид уровня: камера, слои показа, выбор объекта щелчком и рамкой** (`level.view`): `LEVEL=…;eye=;look=;yaw=;pitch=;fov=;top;grid;stats;collision;…`.
- **Переместить, повернуть, масштабировать объект уровня** (`level.transform`): `LEVEL=…;select=G:O;act=move:dx,dy,dz | act=turn:град (нет масштаба и абсолютной позиции)`.
- **Дублировать объект в его SimGroup** (`level.duplicate`): `LEVEL=…;act=duplicate`.
- **Удалить объект из SimGroup** (`level.remove`): `LEVEL=…;act=remove`.
- **Добавить объект по образцу: с моделью образца, другой моделью или её клоном со своей коллизией** (`level.add`): `LEVEL=…;act=add:<id>[:clone][:collision][:into=<зона>][:at=X/Y/Z]; WIZARD=place,model=,zones=`.
- **Поставить модель из файла на уровень одним действием: новый набор в потоке зоны, своя коллизия, объект на земле** (`level.place-file`): `LEVEL=…;act=file:<файл>[:at=X/Y/Z][:name=][:collision=mesh|hull|box|none][:limit=<px>]`.
- **Добавить объект класса с пустыми атрибутами (без образца)** (`level.add-empty`): `LEVEL=…;act=empty:<класс>[:into=<зона>]`.
- **Добавить в поток новую SimGroup: пустую или с копиями объектов другой** (`level.simgroup-new`): `ZONE=simgroup:stream=[,copy=<поток>#<ресурс>][,objects=0+1]`.
- **Добавить поток в мировую сетку уровня (wgi): игра грузит его у игрока** (`level.grid-add`): `ZONE=cell:level=,stream=,min=X/Y/Z,max=X/Y/Z[,lod=][,name=][,on_enable]`.
- **Мировая сетка уровня: показать ячейки, убрать ячейку потока** (`level.grid-info`): `ZONE=remove-cell:level=,stream= (список — только в окне)`.
- **Править навигационный граф (пути ИИ): узлы, связи, флаги, R-дерево** (`level.navgraph`): `LEVEL=…;nav=G:N;act=nav-rtree | act=nav-flags:<hex> (узлы и связи — только мышью и полями)`.
- **Пройти по уровню и проверить маршрут из A в B по коллизии** (`level.walk`): `LEVEL=…;walk;route=ax,ay,az,bx,by,bz`.
- **Граф логики: объекты, пины, сообщения, кто шлёт и кто слышит; миссии** (`logic.graph`): `LOGIC=<область>;find;show`.
- **Прогнать логику миссии без игры: сообщения, переменные, таймеры, Lua** (`logic.run`): `LOGIC=…;run=<события>;until=<с>`.
- **Прочитать скрипт Lua игры: декомпиляция, листинг байткода** (`lua.read`): `LUA=…`.
- **Визуальный скрипт: создать, изменить, поставить в уровень, убрать** (`script.deploy`): `SCRIPT=<graph.json> открывает граф, но не ставит`.
- **Изменить текст строки** (`text.edit`): `WIZARD=texts,table=,find=,replace=,replace_all[,everywhere]; RESOURCE=[language=<код>,]string=<имя или 0xхеш>:<текст> (запятая в значении — \, или значение строкой JSON)`.
- **Добавить новую строку в таблицы языков с переводами** (`text.add`): `RESOURCE=add_string=<имя>:<текст>[,tr=<код языка>:<текст>…]`.
- **Ссылки $name ролика интерфейса на строки: показать, перенаправить** (`text.apx-refs`): `RESOURCE=apx=<старое имя>:<новое имя>`.
- **Диалоги: список, поиск, дерево реплик с речью и текстами** (`dialogue.read`): `DIALOGUE=…`.
- **Изменить текст субтитров речевого файла архива по языкам** (`subtitle.text`): `SOUND=select=<файл>,subtitle=<язык>:<текст>,apply`.
- **Изменить начало и длину субтитра** (`subtitle.timing`): `CUTSCENE=select=sub:N только выбирает`.
- **Заменить потоковый звук или речь архива .viv** (`sound.replace-stream`): `SOUND=select=<файл>,replace=<звуковой файл>`.
- **Потоковые звуки и речь архивов .viv: список, сведения, экспорт в WAV** (`sound.archives`): `SOUND=filter=<текст>,select=<файл>,export=<file.wav>`.
- **Новая реплика из записи: звук, запись речевого банка, строка набора говорящего, событие** (`sound.voice-line`): `WIZARD=voice,chv=,file=,event=,name=,subtitle=,only`.
- **Речевые банки и таблица событий речи: показать, добавить запись, изменить паузу и приоритет** (`sound.speech-tables`): `SOUND=page=banks,filter=<текст>; SOUND=event=<ИМЯ>,cooldown=,priority=,flags=,rank=,apply`.
- **Аудиоконфигурация и снимки микшера: категории, громкость, фильтр** (`sound.mix`): `SOUND=category=<ИМЯ>,parent=<ИМЯ>,apply; SOUND=mix=<имя>,channel=<КАТЕГОРИЯ>,volume=,time=,frequency=,apply`.
- **Ролики на движке: список кадров, шкала времени, актёры, звук, субтитры; предпросмотр** (`cine.read`): `CUTSCENE=time,play,preview,…; CINE=time=,characters,through,view=`.
- **Команды кадра на шкале времени: текст на экране, время суток, сообщение, тени, масштаб времени** (`cine.commands`): `CUTSCENE=select=… только выбирает`.
- **Заменить ролик .vp6: картинка и/или звуковая дорожка** (`video.replace`): `MOVIES=<ролик> только открывает`.
- **База частей персонажей: группы, наборы, слоты, варианты, части** (`char.partsdb`): `CHARACTER=set,lod,choose,select`.
- **Новая часть одежды: модель, материалы с оттенками, часть и вариант, разрешить в наборах** (`char.outfit`): `WIZARD=outfit,slot=,set=,template=<часть-образец>,file=,lod=<файл второго уровня детализации>,name=,tint=<0..2>:<r>/<g>/<b>,tags=,only`.
- **Банки анимаций: клипы, события, граф узлов; экспорт клипов** (`anim.read`): `ANIM=clip,frame,graph,node`.
- **Импортировать клип анимации (заменить или добавить), изменить события клипа и граф узлов** (`anim.edit`): `ANIM=import=<файл>,add=<имя>`.
- **Изменить параметры игры из курируемого списка во всех копиях атрибута** (`params.tunables`): `WIZARD=params,add=,category=,value=,factor=,find=`.
- **Состояние рабочего пространства: изменения, новые потоки, история операций** (`ws.status`): `CHANGES=1; SHELL=drawer=history`.
- **Проект мода: состояние исходных файлов и сборка изменённых в рабочее пространство** (`project.build`): `PROJECT=<папка>`.
- **Создать проект из потоков игры или изменений (с добавленными ресурсами и новыми потоками); записать версию рабочего пространства в файлы проекта; записать исходники всего, что держит рабочее пространство проекта** (`project.new`): `PROJECT=<папка> и SHELL=pull, SHELL=sources`.
- **Проект как документ: папка со своим рабочим пространством — создать пустой, открыть, закрыть** (`project.document`): `OPEN_PROJECT=<папка>`.
- **Собрать файл мода из изменений: имя, id, версия, автор, связи, издания, выбор изменений** (`mods.build`): `WIZARD=package,name=,author=,description=,out= (нет id, версии, издания, связей)`.
- **Менеджер модов: установленные моды, состояние в игре, конфликты (целых ресурсов и по элементам у модов с дельтами), совместимость** (`mods.read`): `MODS=1`.
- **Что в файле мода (.zip, .vpat) до установки (ресурсы, дельты и число операций в них, потоки, связи, код), и его проверка** (`mods.info`): `MODS=info=<файл>`.
- **Записать включённые моды в игру и снять их** (`mods.apply`): `DOCTOR_TOOLKIT_MODS_ASK=write открывает вопрос`.
- **Записать установленный мод файлом мода** (`mods.export`): `MODS=export=<id>:<файл>`.
- **Запустить игру с правками (обычно или сразу в потоке уровня у точки появления) и остановить** (`play.start`): `SHELL=play только открывает окно параметров`.
- **Отчёты о падениях игры: список, показать, состояние модуля, адрес по базе знаний** (`crash.reports`): `SHELL=crashes[=<номер или имя отчёта>]`.
- **Связь с запущенной игрой: есть ли она, состояние цикла, загруженные потоки** (`live.status`): `LIVE=PORT[:TOKEN]`.
- **Править мир на ходу: сдвинуть, повернуть, скрыть объект, включить и выключить его логику, отменить** (`live.object-edit`): `LIVE=PORT[:TOKEN] и LIVE_DO=get=<guid>,hide=<guid>,show=<guid>,logic=<guid>:on|off,undo (без вопроса: переменная — согласие)`.
- **Измерить игру изнутри: время кадра, ЦП потоков, чтения файлов, текстуры, память** (`live.profile`): `LIVE=PORT[:TOKEN] и LIVE_DO=profile[=reset+memory+sample:<мс>]`.
- **Свободная камера: игра рисует из заданной точки; камера редактора ↔ камера игры** (`live.freecam`): `LIVE_FOLLOW=1 (только следование)`.
- **Отладка вживую: пауза, шаг, точки останова на сообщении, печати и ошибке Lua; логика и скрипты вживую** (`live.debug`): `LOGIC=…;live`.
- **Проверить правки перед записью в игру: ресурсы читаются, модели подходят классам, ссылки сходятся, сообщения, конфликты** (`check.validate`): `SHELL=check (ошибки в правках — ненулевой код выхода)`.
- **Проверить ссылки потоков игры: всё названное лежит в потоке или в загруженном раньше** (`check.refs`): `SHELL=refs=<поток или папка>`.
- **Заглянуть в файл сохранения: сводка, переменные, деньги, семьи, здания (только чтение)** (`save.read`): `SAVE=<файл>`.
- **База знаний: поиск по заметкам и чтение заметки** (`kb.read`): `SHELL=kb=<слова через +> | kb=<заметка.md>`.
- **Типизированные данные игры: списки значений, сведения о сообщении, смысл атрибутов класса** (`kb.game-types`): `SHELL=types=<список> | types=message:<текст> | types=class:<имя>`.
- **Окно редактора: панели, вкладки, раскладки, темы, масштаб, настройки, справка по клавишам** (`shell.window`): `SHELL=prefs,about,shortcuts,welcome,log,layout=,drawer=,max,…; SHOT=<файл> — снимок окна и выход`.

## Что нужно, чтобы закрыть дыры

Оценка объёма: малая — до дня работы над готовой функцией библиотеки; средняя — несколько дней, новая панель или перенос логики в библиотеку; большая — затрагивает устройство библиотеки или редактора. Причины блокирующих находок и разбор общего слоя операций — в [разборе паритета](analysis/parity.md).

### Редактор догоняет MCP (этап 103)

| Пакет | Что | Объём | Почему | Зависит от | Стенд | Дыр |
|---|---|---|---|---|---|---|

### MCP догоняет редактор (этап 104)

| Пакет | Что | Объём | Почему | Зависит от | Стенд | Дыр |
|---|---|---|---|---|---|---|
| 104-A | Ролики в MCP: вывод ролика в видео (кадры, камеры, ключи, актёры, команды и время субтитров сделаны) | средняя | рендер делает программа редактора без окна; серверу нужна схема долгих задач | — | нет | 1 |
| 104-P | База проекта (этап 108) в MCP и Python: записать базу из модов игры и привести моды к базе | малая | библиотека готова (project::base: record, differences, bring); нужны параметры инструмента и метод Python | — | нет | 1 |
| 104-F | Модель из файла на уровень одним вызовом MCP (level::place: образец, импорт в поток зоны, объект — одна операция) | малая | библиотека готова (level::place::samples, plan, record); образец по точке выбирает она же, нужен инструмент | — | нет | 1 |
| 104-G | Смысловые эквиваленты действий мышью: несколько объектов, на землю, маршрут, снимок вида | большая | коллизия для ходьбы и отрисовка вида живут в крейте редактора (rapier, wgpu): для инструмента их надо вынести или дать редактору работать без окна | — | нет | 2 |

**104-A — Ролики в MCP: вывод ролика в видео (кадры, камеры, ключи, актёры, команды и время субтитров сделаны)**

- **Вывести ролик: видео через камеры кадров, glTF с камерой и актёрами** (`cine.export`). Добавить: инструмент cutscene_render: видео через камеры кадров — долгая задача с опросом состояния. Объём: средняя — рендер делает программа редактора без окна (--render-cutscene, нужна видеокарта): серверу нужна схема долгих задач и путь к этой программе. Стенд игры: нет.

**104-P — База проекта (этап 108) в MCP и Python: записать базу из модов игры и привести моды к базе**

- **База проекта — моды, на которых он сделан: записать из игры, сверить с игрой, привести моды к базе** (`project.base`). Добавить: действия base_record и base_apply инструмента проекта (project::base: ProjectConfig::record_base, Base::bring). Объём: малая — библиотека готова (этап 108), нужны параметры инструмента. Стенд игры: нет.

**104-F — Модель из файла на уровень одним вызовом MCP (level::place: образец, импорт в поток зоны, объект — одна операция)**

- **Поставить модель из файла на уровень одним действием: новый набор в потоке зоны, своя коллизия, объект на земле** (`level.place-file`). Добавить: инструмент place_model: файл, точка, имя — через level::place. Объём: малая — библиотека готова; образец по точке выбирает level::place::samples. Стенд игры: нет.

**104-G — Смысловые эквиваленты действий мышью: несколько объектов, на землю, маршрут, снимок вида**

- **3D-вид уровня: камера, слои показа, выбор объекта щелчком и рамкой** (`level.view`). Добавить: инструмент level_view: снимок вида уровня из точки с правками рабочего пространства — запуском редактора без рук на своём X-сервере (предустановки LEVEL=…;eye=;look= и CAPTURE) либо запросом к открытому редактору через мост. Объём: большая — отрисовка живёт в крейте редактора (wgpu) и требует окна или своей поверхности; коллизия и лучи уже в библиотеке (level::walk), ground и walk_route отвечают на вопросы «что под точкой» и «пройдёт ли игрок» без картинки. Стенд игры: нет.
- **Ролики на движке: список кадров, шкала времени, актёры, звук, субтитры; предпросмотр** (`cine.read`). Добавить: снимок кадра ролика: вместе со снимком вида уровня (level.view). Объём: большая — отрисовка живёт в крейте редактора (wgpu) и требует окна или своей поверхности. Стенд игры: нет.
