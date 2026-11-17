---
type: question
status: unknown
confidence: guess
editions: ["1.0.789.0"]
stages: [81]
sources: ["docs/mods.md §8", "crates/doctor-toolkit-loader/src/lib.rs", "docs/analysis/game-bench.md §45"]
tags: [question, native-mods, loader, windows, tooling]
updated: 2026-11-16
---
# Код модов и загрузчик тулкита на Windows

[Загрузчик тулкита](../engine/asi-loader.md) и [установка кода мода](../engine/native-mod-install.md) проверены только под Wine. На Windows не проверено:

- загрузка модулей из `DllMain` загрузчика (под блокировкой загрузчика Windows) — модули тулкита в своих `DllMain` только патчат код и создают потоки, но на настоящей Windows это не запускалось;
- удаление DLL, пока игра запущена: Windows держит файл открытым, менеджер оставляет запись и убирает файл в следующий раз — этот путь проверен только тестом с папкой без права записи (`a_module_that_cannot_be_removed_now_goes_the_next_time`), не занятым файлом;
- имена файлов без учёта регистра и антивирусы, которые могут задержать или удалить DLL мода.

Догадка: всё это работает как под Wine; источника нет.

## Следующий шаг

Прогнать `research/bench_native_mod.sh` (шаги `install` и `remove`) и запуск игры на Windows с примером `examples/native-class`; при отказе загрузки из `DllMain` — перенести загрузку модулей на первый вызов перехваченной функции старта, как делает Ultimate ASI Loader.

## Связанное

- [Моды и менеджер модов](../../mods.md) §1.6, §8
