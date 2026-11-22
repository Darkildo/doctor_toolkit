---
type: experiment
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [119]
sources: ["docs/analysis/game-bench.md §64", "PLAN-cheats.md §1, §3", "crates/doctor-toolkit-cheats-asi/src/module.rs", "crates/doctor-toolkit-cheats-asi/src/view.rs", "tools/build-cheats-module.sh"]
tags: [bench, native-mods, overlay, input, fonts, mod-manager, money]
updated: 2026-11-20
---
# Меню разработчика `doctor_cheats` на стенде (этап 119)

Проверяли мод `doctor_cheats.asi` — нативный мод на платформе: меню поверх кадра по `~`, стрелки с
автоповтором, Enter, Esc; страницы Player, Weapons, Crew, Families. Работает один пункт — «Money +$10 000»
(`AddMoney` семьи `corleone`); остальные тусклые, «soon». Стенд: Wine, DXVK, Xvfb, 1024×768, модуль связи,
модуль падений и графический модуль с FXAA; мод примера не ставили.

- **Менеджер модов ставит мод.** `tools/build-cheats-module.sh` собирает мод формата 5 с модулем в
  `native`. `mods install` показывает код и ничего не пишет, `mods native --allow` — согласие, `mods apply`
  пишет `doctor_cheats.asi` и `doctor_platform.asi`, `mods unapply` убирает оба.
- **Строка журнала платформы:** «doctor_cheats 0.1.0 on PC 1.0.789.0 …: key 0x29 (reserved 0), pad true,
  language En, fonts Some((1024, 512)), tick 1, overlay 2». Весь `doctor_mod_init` с атласом — 9–12 мс.
- **Язык** решил `settings.txt` префикса (`AppData/Local/EA Games/Крестный отец™ II/`): с `Locale = en` —
  En, с `Locale = ru` — Ru. Ключа реестра в префиксе нет. `language = en|ru` в `doctor_cheats.cfg` берёт верх.
- **Уровень:** пока меню открыто, игрок не идёт и камера стоит. Курсор ходит и переходит через край списка;
  удержание 1,6 с — 17 шагов, первый повтор через ~0,4 с. «Money» даёт +10 000: `live money` 0 → 10000 →
  20000, строка «Money: $N» и HUD «$20,000». Тусклый пункт — «Not in this version yet» на 3,5 с. Первый
  `Escape` — назад, второй закрывает; меню паузы не открывается.
- **Меню паузы и «Взгляд дона»** (режим такта 1): меню над ними, деньги работают (30 000, 40 000). Вне
  панели кадр паузы тот же до точки, после закрытия меню паузы на месте. Кнопка «Continue» «Взгляда дона»
  при открытом меню не нажимается. Счётчик денег «Взгляда дона» обновляется после «Continue».
- **Главное меню:** «Not available here» — и с заставки, и после выхода из уровня (список семей пуст).
  `Return` и `Escape` до меню игры не доходят. `~` не проходит «Press any key» ни при открытии, ни при
  закрытии.
- По-русски все страницы читаются, `ё`, `—`, `·`, `×` на месте.
- Исключений, ошибок Lua и отчётов о падении нет; `doctor_render` без «the game made a state block».
- Мелочи: строка результата пишет «$10000» без разделителя (`module.rs:196`). Русская подсказка
  (`text.rs:78`) почти во всю ширину панели: на 3 пикселя за отступом, до рамки 18, переноса нет. Открытие и
  закрытие меню в журнал не пишутся.

Не проверено: геймпад (Back + RB, крестовина, A, B), другая клавиша и `pad = off`, экраны загрузки,
режимы выше 1024×768, 16:9 / 21:9, окно, родной d3d9.

## Подробнее

- [Стенд](../../analysis/game-bench.md) §64
- [Рисование и шрифты оверлея (этап 118)](platform-overlay-painter.md)
- [Шлюз ввода платформы (этап 117)](platform-input-gate.md)
- [Нативный мод в менеджере модов](native-mod-in-manager.md)
- [Платформа нативных модов](../engine/native-mods.md)
