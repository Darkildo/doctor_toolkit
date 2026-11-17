---
type: engine
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [81]
sources: ["research/exe_imports.py", "crates/doctor-toolkit-loader/src/lib.rs", "crates/doctor-toolkit/src/live/loader.rs", "docs/analysis/game-bench.md §45"]
tags: [asi, loader, native-mods, tooling]
updated: 2026-11-16
---
# ASI-загрузчик: как DLL попадают в процесс игры

Модули `.asi` — обычные DLL. В процесс игры их грузит ASI-загрузчик: DLL в папке игры с именем системной, которую Windows находит раньше системной.

## Что известно

- `godfather2.exe` берёт из `DINPUT8.dll` одну функцию — `DirectInput8Create` (таблица импорта, `research/exe_imports.py`). Поэтому загрузчику под именем `dinput8.dll` достаточно передать её системной DLL.
- У игроков этой игры загрузчик — Ultimate ASI Loader (чужая программа): грузит `*.asi` из папки игры, `scripts` и `plugins`. Тулкит его не поставляет и не скачивает, а узнаёт по имени и меткам (`live::loader::find`).
- Загрузчик тулкита `doctor_loader.dll` (крейт `doctor-toolkit-loader`) делает то же: при загрузке DLL грузит `*.asi` из тех же трёх папок по алфавиту, `DirectInput8Create` передаёт `dinput8.dll` из системной папки. Ничего не патчит, настроек не читает, файлов не пишет.
- Модули грузятся из `DllMain` загрузчика, то есть до точки входа игры: модули тулкита ставят свои перехваты старта приложения из своих `DllMain`.
- Менеджер модов кладёт его как `dinput8.dll`, только когда загрузчика в папке игры нет и мод с кодом разрешён игроком; убирает, когда в папке игры и `scripts` не осталось ни одного `.asi` — [правила](native-mod-install.md).
- На стенде (Wine, `WINEDLLOVERRIDES=dinput8=n,b`) загрузчик тулкита загрузил пять модулей: платформу, два мода примера, модуль связи и модуль падений; игра дошла до уровня, игрок ходит с клавиатуры — [опыт](../experiments/native-mod-in-manager.md).
- Под Wine и Proton без `WINEDLLOVERRIDES=dinput8=n,b` берётся встроенная `dinput8.dll`, и никакой загрузчик не работает.

## Что неизвестно

- [Работа на Windows](../questions/native-mods-on-windows.md).

## Подробнее

- [Моды и менеджер модов](../../mods.md) §1.6
- [Платформа нативных модов](native-mods.md), [модуль `doctor_live`](live-module.md)
