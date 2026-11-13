# Python-привязки

Этап 12: модуль `doctor_toolkit` для исследовательских
ноутбуков и пакетных скриптов модов. Привязки тонкие: каждый объект оборачивает
тип библиотеки, каждый метод вызывает одну функцию библиотеки, поэтому формат
данных, проверки и сообщения об ошибках те же, что у CLI и GUI.

Код: `crates/doctor-toolkit-py/` (крейт `pyo3`, `cdylib`), пакет
`crates/doctor-toolkit-py/python/doctor_toolkit/` (реэкспорт, `testing`,
заглушки типов `_native.pyi`), тесты `crates/doctor-toolkit-py/tests/`
(pytest), примеры `examples/python/`.

## 1. Сборка

Нужны Rust (тулчейн проекта) и Python ≥ 3.9. Модуль собирается `maturin` в
стабильном ABI (`abi3`): одно колесо подходит ко всем CPython от 3.9.
Заголовки Python не нужны.

```sh
python3 -m venv .venv && . .venv/bin/activate
pip install maturin pytest
cd crates/doctor-toolkit-py
maturin develop --release          # собрать и поставить в .venv (редактируемо)
maturin build --release            # колесо в target/wheels/
pytest                             # тесты на синтетической игре
EARS_GF2_DIR="/path/to/The Godfather II" pytest   # плюс тесты на файлах игры
```

Если активны и conda, и venv, `maturin` просит оставить одно: `env -u
CONDA_PREFIX maturin develop`.

Крейт входит в workspace, но не в `default-members`: `cargo build` и `cargo
test` его не собирают, `--workspace` собирает. В `.cargo/config.toml` заданы
`PYO3_BUILD_EXTENSION_MODULE=1` и `PYO3_NO_PYTHON=1`: модуль не линкуется с
`libpython` (его загружает интерпретатор) и собирается без интерпретатора,
поэтому `cargo build --workspace`, `cargo clippy --workspace` и `cargo test
--workspace` работают и на машине без Python. Rust-тестов у крейта нет
(`test = false`): тестовый бинарник не слинкуется без `libpython`; тесты
написаны на Python. `build.rs` добавляет флаги линковки модуля для macOS.

## 2. Обзор API

```python
import doctor_toolkit as dt

game = dt.Game("/path/to/The Godfather II")
game.streams                        # ['gf2/cuba', ..., 'gf2/frontend', ...]
stream = game.stream("gf2/frontend")
for r in stream:                    # Resource: key, type, ids, data_size, compressed, ...
    print(r.key, r.type, r.data_size)
data = game.read("gf2/frontend:tg4h:00000000.4802e0a7.f3d257db.9cf57f36")   # bytes
for r in game.resources("simgroup", streams=["gf2/cuba"]):
    group = r.parse()               # dict
```

Ключ ресурса — строка `поток:тип:ids` (`docs/workspace.md` §1); везде, где
нужен ключ, можно передать и `Resource`. Тип — тег (`"tg4h"`), шестнадцатеричный
id (`"0xcdb9d42b"`) или число.

| Объект | Что даёт |
|---|---|
| `Game(root)` | `streams`, `tocs`, `stream(name)`, `resources(type, streams)` (ленивый итератор по потокам), `resource(key)`, `read(key)`, `toc(path)`, `index()`, `workspace()`, `init_workspace()`, `mods()`, `unpack()`, `pack()`, `integrity()`, `archive_entries()`, `archive_sound(name)` |
| `Stream` | отображённый в память файл: `len()`, индексация, итерация, `resources(type)`, `find(type, id)`, `get(key)`, `read(i)`, `textures()` |
| `Resource` | заголовок (`key`, `type`, `type_id`, `ids`, `id`, `data_size`, `stored_size`, `compressed`, `mem_policy`), `read()`, `parse()`, `texture()`, `string_table()`, `model()`, `to_dict()` |
| `Texture` | `name`, `package`, `width`, `height`, `format`, `kind`, `mip_count`, `layer_count`, `params`, `keys`; `to_png(layer)`, `to_dds()`, `rgba(layer)` → `(w, h, bytes)`, `replaced_by(png_or_dds)` |
| `StringTable` | `items(language)`, `get`/`set`, `t[hash]`, `t[hash] = text` (во всех языках), `to_dict()`, `to_bytes()`, `StringTable.parse(bytes)` |
| `Model` | `records`, `vertices(i)`, `triangles(i)` — упакованные буферы; `to_glb(index=None)`, `export(path, index=None, format=None)` — glb, fbx, obj, usd, usda, usdc, usdz |
| `Font` | `glyphs`, `metrics`, `atlas` (`Texture`), `to_dict()` |
| `Sound` | `sample_rate`, `channels`, `duration`; `to_wav()`, `pcm()`, `to_snu()` |
| `Index` | `search(query, limit)`, `count(query)`, `record(key)`, `refs(key)`, `keys(type, stream)`, `lookup(hash)`, `refresh()` |
| `Workspace` | `changes`, `history`, `read`/`data`/`original_data`, `set_data`, `set_many`, `import_texture`, `import_model`, `export_model`, `import_animation`, `set_clip_events`, `set_animation_graph`, `string_table`, `set_string_table`, `edit_strings`, `revert`, `undo`/`redo`, `status()`, `diff()`, `apply()`, `unapply()`, `build_mod()` |
| `ModManager` | `mods`, `install`, `install_workspace`, `uninstall`, `enable`, `move`, `conflicts`, `clashes`, `check`, `status`, `apply`, `unapply`, `export` |

Функции модуля: `sdbm`, `crc32`, `fnv1a` (строка или байты), `hash_all(text)`,
`type_id(tag)`, `type_name(id)`, `lookup(hash)` (встроенные списки имён),
`parse_key(key)`, `decode(type, bytes)` (разбор данных без игры),
`sound_from_snu(bytes)`, `animation_bank(bytes)` (клипы `bnk` с событиями),
`animation_graph(bytes)` (входы и деревья узлов банка, проверки, число слотов;
`docs/formats/anim-graph.md`),
`animation_glb(...)` (клипы в glTF, `docs/formats/characters.md` §6), `mod_info(path)`, `testing.build_sample_game(dir)`.

Полные сигнатуры и описания — в `python/doctor_toolkit/_native.pyi`
(подхватываются IDE и `mypy`) и в docstring-ах (`help(dt.Workspace)`).

### 2.1 Разобранные ресурсы

`Resource.parse()` выбирает разбор по типу:

| Тип | Результат |
|---|---|
| `tg4h`, `tg4d` | `Texture` (пара заголовок + данные) |
| `lh2` | `StringTable` |
| `mesh` | `Model` (вместе с `meshvolatile`) |
| `ffn` | `Font` с атласом из того же потока |
| `sbk` | `list[Sound]` |
| `simgroup`, `metamodel`, `variabledictionary`, `partsdb`, `graph`, `mobface` | dict — тот же JSON, что `logic-export` |
| `rcb`, `lua`, `hkx`, `bnk`, `tob` | dict — сериализация библиотеки (`bnk`: клипы и события без кадров) |
| `material` | dict: `name`, `name_hash`, `shaders`, `shadow_shader`, `params`, `shading` (шейдер, семейство, текстуры, оттенки — `docs/formats/characters.md` §2) |
| `apx` | dict: `title`, `blocks`, `textures` (`list[Texture]`) |
| прочие | `InvalidArgumentError` «no parser» |

Объекты SimGroup в dict несут `class`, `class_name`, `guid`, `name`,
`position` (строка переноса матрицы `CMD_LoadMatrix`), `model` (id metamodel),
`packets` с атрибутами. Хэши и GUID — шестнадцатеричные строки, как в JSON CLI.

Скрипты Lua (`docs/formats/lua.md`):

```python
out = dt.lua_decompile(res.data())      # source, round_trip, detail, errors
chunk = dt.lua_compile(out["source"].replace("false", "true", 1),
                       dt.decode("lua", res.data())["source"])
ws.set_data(res.key, chunk, "Edit script")
print(dt.lua_listing(chunk))            # инструкции
```

`round_trip` — `"identical"` (перекомпиляция побайтно равна чанку),
`"same_code"` (те же инструкции, другие строки отладки) или `"different"`.
`lua_compile(source, chunk_name="=script", number_size=4)`; 8 — числа
`double` стандартного Lua.

Граф логики миссий (`docs/logic-graph.md`):

```python
g = dt.logic_graph(game, "mission:killingcastro")    # области как у logic-graph
g["overview"]["states"]                               # connected, never_sent, …
for w in g["wires"]:
    src = g["nodes"][w["from"]["node"]]
dt.logic_messages(game, "cp_737_loaded")              # senders, listeners или None
dt.logic_messages(game, "cp_737_", search=True)       # список сообщений
dt.logic_missions(game)                               # [{"name", "streams"}]
dt.logic_schema()                                     # покрытие схемы пинов
r = dt.logic_run(game, "checkpoint:737", events=["load", "send:cp_737_loaded"], until=60)
r["journal"]                                          # кто кому послал (logic-graph.md §9)
```

`logic_graph(game, scope, hubs=True, all=False, original=False)`: `hubs` —
сообщения с большим числом концов через узел-хаб, `all` — оставить объекты
без сообщений, `original` — без правок рабочего пространства. Строится
индекс игры (около секунды), граф миссии — доли секунды.

Визуальные скрипты (`docs/scripts.md`): `script_compile(graph_json)` →
`source` (Lua), `chunk` (байты для игры или `None` при ошибках) и
`diagnostics` (словари `severity`, `node`, `pin`, `message`).

### 2.2 Данные и numpy

Данные возвращаются как `bytes`. Распакованные данные ресурса — новый буфер
(RefPack и разбиение на блоки не дают отдать кусок файла без копии), копия
одна: из буфера библиотеки в `bytes`. Пиксели и вершины удобно смотреть через
numpy без ещё одной копии:

```python
w, h, rgba = texture.rgba()
img = numpy.frombuffer(rgba, numpy.uint8).reshape(h, w, 4)
v = model.vertices(0)
pos = numpy.frombuffer(v["positions"], "<f4").reshape(v["count"], 3)
tri = numpy.frombuffer(model.triangles(0), "<u4").reshape(-1, 3)
pcm = numpy.frombuffer(sound.pcm(), "<i2").reshape(-1, sound.channels)
```

Протокол буфера (memoryview поверх отображённого файла) в стабильном ABI
доступен только с Python 3.11, поэтому не используется.

### 2.3 Ошибки

Ошибки библиотеки — исключения с её сообщением:

| Исключение | Ошибка библиотеки |
|---|---|
| `DoctorToolkitError` | базовый класс всех ниже |
| `FormatError` | `BadMagic`, `UnexpectedEof`, `UnsupportedVersion`, `Malformed`, `Refpack` |
| `InvalidArgumentError` | `InvalidArgument`: неверный ключ, отказ записи (поток изменён другой программой); `NotAGame`: каталог не содержит файлов игры |
| `ModError` | `Mod`: повреждённый мод, отказ применения (мод не подходит, рабочее пространство в игре) |
| `IoError` | `Io`: сообщение содержит путь |
| `LuaSyntaxError` (подкласс `InvalidArgumentError`) | синтаксическая ошибка `lua_compile`; `args` — `(сообщение, строка)` |

Ошибки поиска по имени — стандартные: нет потока или ресурса — `KeyError`,
индекс вне потока — `IndexError`, неверный аргумент Python — `TypeError` /
`ValueError`.

### 2.4 GIL и потоки

Долгие операции отпускают GIL: открытие игры, чтение и распаковка данных,
кодирование PNG/DDS, разбор логики, экспорт glTF, построение и обновление
индекса, поиск, `apply`/`unapply` рабочего пространства и модов, установка и
сборка модов, `unpack`/`pack`, проверка целостности. Их можно запускать из
нескольких потоков Python (`concurrent.futures.ThreadPoolExecutor`). Модуль
объявлен совместимым со свободнопоточным Python. Объекты `Workspace`,
`ModManager`, `StringTable` и `Index.refresh` изменяемые: одновременный вызов
изменяющих методов одного объекта из двух потоков даёт `RuntimeError`
(«already borrowed»), а не гонку.

## 3. Индекс и поиск

```python
index = game.index()                # кэш в ~/.cache/doctor-toolkit/, как у CLI
index.search("type:material rust", limit=20)
index.search("text:Don Corleone")
index.refs("gf2/frontend:tg4h:00000000.4802e0a7.f3d257db.9cf57f36")
index.lookup(0x38523FC3)            # [('Entity', 'sdbm(lower)')]
model.to_glb(index)                 # glTF с материалами, текстурами и скелетом
model.export("model.fbx", index)   # или .obj, .usda, .usdc, .usdz (docs/model-formats.md)
dt.read_model("model.fbx")          # части, материалы, суставы, клипы, как их видит импорт
```

Синтаксис запроса — как у `doctor-toolkit search` (`docs/navigation.md` §3).
`game.index(cache=False)` строит индекс без кэша, `cache="path.bin"` —
с кэшем в указанном файле. Первое построение по всей игре читает ~1,2 ГБ;
дальше читаются только изменённые потоки. `index.refresh()` после `apply`
перечитывает изменённые потоки.

## 4. Рабочее пространство и моды

```python
ws = game.workspace()                          # <game>/_doctor_toolkit/workspace
ws.edit_strings("gf2/text/dd7d7050:lh2:...", {0x1234abcd: "Новый текст"})
ws.import_texture(texture_key, open("new.png", "rb").read())
ws.set_data(key, new_bytes, label="My change")
ws.status(); ws.diff(); ws.undo(); ws.redo()
ws.apply()                                     # запись в игру
ws.unapply()                                   # оригиналы обратно, правки остаются
ws.build_mod("my-mod.zip", "My mod", author="me")
mods = game.mods()
mods.install("my-mod.zip"); mods.apply(); mods.unapply()
```

Правки записываются так же, как CLI: в рабочее пространство, а в игру — только
`apply()`. Проверки те же, потому что их делает библиотека:

- поток, изменённый другой программой, `apply` не перезаписывает без
  `force=True` (`InvalidArgumentError`);
- рабочее пространство не пишет в игру, пока там моды менеджера, и наоборот
  (`ModError`);
- мод, чьи оригиналы не совпадают с игрой, применяется только с
  `ignore_compatibility=True`;
- перед первой записью файл копируется в `_doctor_toolkit/backups`, файлы
  заменяются атомарно, размеры в TOC обновляются.

`set_many({key: bytes})` записывает правки каждого потока одной операцией.
`revert(["часть ключа"])` убирает правки по ключу или его части, как CLI.

Открытый `Stream` держит отображение старого файла: после `apply` он
показывает прежние данные (файл заменён атомарно, на Linux старое отображение
остаётся валидным). Для новых данных откройте поток заново или создайте новый
`Game`. На Windows открытый поток мешает замене файла — закрывайте потоки
(`del stream`) до `apply`.

### 4.1 Проект (этап 36)

Мод как папка исходников (`docs/project.md`): те же действия, что
`doctor-toolkit project new/status/build/pull/mod`.

```python
project = dt.Project.create(game, "my_mod", name="My mod",
                            streams=["gf2/frontend"], index=game.index())
project.status()            # {"entries": [{"id", "state", ...}], "to_build": 0, ...}
# … правка файлов в my_mod/src/… …
report = project.build()    # только изменённое, одной операцией рабочего пространства
report["errors"]            # ["src/…/x.json: not valid JSON: …"] — остальное собрано
project.build_mod("my-mod.zip")
project = dt.Project("my_mod")                  # открыть; игра — из project.toml
dt.project_object_guid("my-mod:simgroup:00000004", "lamp")   # GUID нового объекта
```

`Project.create(..., from_workspace=True)` берёт ресурсы, которые меняет
рабочее пространство; `project.pull()` переносит в файлы правки, сделанные
мимо проекта (состояние `out of sync`).

## 5. Тесты

`crates/doctor-toolkit-py/tests/`:

| Файл | Что проверяет |
|---|---|
| `test_sample_game.py` | хэши, ключи, потоки, ресурсы, текстура (пиксели), таблица строк, SimGroup, ошибки, индекс и поиск, вызовы из нескольких потоков — на синтетической игре |
| `test_workspace_mods.py` | запись в рабочее пространство без изменения игры, diff, undo/redo, apply/unapply, отказ при чужом изменении и `force`, импорт PNG, моды (сборка, установка, конфликты, взаимное исключение с рабочим пространством), `unpack`/`pack`, целостность, `refresh` |
| `test_project.py` | проект на синтетической игре: создание, состояние, сборка только изменённого, мод, ошибка файла с путём, GUID новых объектов |
| `test_examples.py` | три скрипта `examples/python/` |
| `test_corpus.py` | только с `EARS_GF2_DIR`: все типы на файлах игры (текстуры, строки, логика, модель в glb с ассетами, риг, шрифт, звук, Lua, архивы `.viv`, индекс 212 544 ресурса) и запись на копиях четырёх файлов во временном каталоге; после теста проверяется, что файлы игры не изменились |

Синтетическая игра — `doctor_toolkit.testing.build_sample_game(dir)`: её
пишет Rust-код библиотеки (`StreamWriter`, `Tg4Header`, `StringTable`, `Toc`),
без данных игры. Кэш индекса в тестах уходит во временный каталог
(`DOCTOR_TOOLKIT_CACHE_DIR`).

## 6. Примеры

`examples/python/`:

| Скрипт | Что делает |
|---|---|
| `export_textures.py GAME STREAM OUT [--dds]` | все текстуры потока в PNG (по слою на файл у кубических карт) или DDS |
| `simgroup_positions.py GAME CLASS [--stream S] [--csv]` | объекты SimGroup класса (`Entity` или хэш) с позициями и моделью |
| `replace_strings.py GAME OLD NEW [--stream S] [--apply] [--dry-run]` | замена текста во всех таблицах строк через рабочее пространство |

## 7. Открытые вопросы

- Изменение SimGroup и APX из Python ждёт этапа 6 (запись этих форматов в
  библиотеке); пока можно заменить данные целиком через `set_data`.
- Добавление ресурсов и потоков — этап 5.
- Прогресс долгих операций в Python не передаётся (нет обратного вызова).

## Персонажи и анимации (этап 17)

```python
import doctor_toolkit as dt
game = dt.Game(GAME)
DB = "gf2/globalfolder/characters/allcharacters"
glb = dt.character_glb(game, DB, "wrk_male", "wrk_0014_cook",
                       choices=["wrk_0014=b_teal_blue"], lod=0)     # персонаж из partsdb
clips = dt.animation_bank(game.stream("gf2/globals_global")[2657].read())["clips"]
graph = dt.animation_graph(game.stream("gf2/globals_global")[2657].read())
idle = next(e for e in graph["entries"] if e["name"] == "idle")  # дерево узлов входа
# ws.set_animation_graph(key, {"entries": graph["entries"]})     # правка графа, одна операция
anim = dt.animation_glb(game, "gf2/globals_global", 2657, clips=["idle"],
                        database="player", set="_all")              # клип на персонаже
fbx = dt.character_glb(game, DB, "wrk_male", "wrk_0014_cook", format="fbx")  # или usdz, usda…
```

`choices` — как `--choose` у `doctor-toolkit partsdb character`:
`"слот=вариант"`, `"пресет=элемент"`, `"tag=тег"`. Обе функции строят
индекс ресурсов игры (несколько секунд). Подробности —
`docs/formats/characters.md`.
