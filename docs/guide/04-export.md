# 4. Экспорт

Экспорт только читает игру. Все команды пишут в каталог, указанный `-o`.

## 4.1 Текстуры

```sh
doctor-toolkit textures "$GAME" -o textures                  # все текстуры как DDS (без потерь)
doctor-toolkit textures "$GAME" -o textures --png            # PNG, только самый крупный mip-уровень
doctor-toolkit textures "$GAME/gf2/frontend.str" -o ui --name button
```

DDS сохраняет формат игры (DXT1, DXT5, RGBA8…) и все mip-уровни — это лучший
выбор, если текстура потом вернётся в игру. PNG удобен для просмотра и правки
в обычных редакторах.

В редакторе: выберите текстуру → **Export DDS…** или **Export PNG…**.

## 4.2 Модели

```sh
doctor-toolkit models "$GAME" gf2/globalfolder/characters/allcharacters -o models --name head
```

Формат — `--format glb` (по умолчанию), `fbx`, `obj`, `usd`, `usda`, `usdc`
или `usdz` (`docs/model-formats.md`); OBJ не хранит скелет. В редакторе — у
модели выбор формата и **Export model…**.
Модель сохраняется как binary glTF (`.glb`) с материалами, текстурами и, для
персонажей, скелетом и весами костей. Файл открывается в Blender
(File → Import → glTF 2.0) и любом просмотрщике glTF. `--no-textures` —
только геометрия.

Скелеты отдельно: `doctor-toolkit skeletons "$GAME" -o rigs` (JSON: кости,
родители, позы).

## 4.3 Звук

```sh
doctor-toolkit audio "$GAME" -o sound --banks-only         # звуки банков sbk как .snu
doctor-toolkit audio "$GAME" -o sound --wav --name ambush  # потоковый звук из .viv, в WAV
```

`.snu` проигрывают vgmstream и foobar2000 с плагином; `--wav` декодирует
EA-XAS в обычный WAV. В редакторе у звукового банка есть кнопка **WAV…**.
Структура банка — `doctor-toolkit sbk-info` (раздел 19).

## 4.4 Тексты, интерфейс, шрифты, скрипты

```sh
doctor-toolkit text-export "$GAME" -o text      # таблицы строк lh2 → JSON (их можно править)
doctor-toolkit apx-export "$GAME" -o ui         # Flash-интерфейсы: .apt, .const и текстуры
doctor-toolkit fonts "$GAME" -o fonts           # атлас PNG и метрики JSON
doctor-toolkit lua-export "$GAME" -o scripts    # байткод Lua 5.0 (.luac)
```

## 4.5 Игровая логика

```sh
doctor-toolkit logic-export "$GAME" -o logic --type simgroup --stream mp_mall
```

Типы: `simgroup` (объекты уровней), `metamodel`, `variabledictionary`,
`partsdb`, `graph`. Результат — JSON для изучения; обратно в игру JSON логики
пока не импортируется.

## 4.6 Распаковка в каталог

`unpack` выгружает потоки целиком в привычных форматах с манифестом —
основной путь для работы во внешних редакторах:

```sh
doctor-toolkit unpack "$GAME" -o work --stream gf2/frontend
```

В `work/` появятся PNG, glTF, JSON, WAV и сырые файлы плюс `manifest.json`.
Как вернуть изменённые файлы — в разделе [5.4](05-editing.md#54-распаковка-и-упаковка).

В редакторе: выделите ресурсы → **Export selected…**; в **All resources**
кнопка экспортирует все показанные ресурсы типа; результаты поиска
экспортируются так же. Экспорт идёт в фоне, его можно отменить.
