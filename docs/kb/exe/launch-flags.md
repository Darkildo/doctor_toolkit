---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 32]
sources: ["docs/analysis/launch-flags.md §3", "docs/analysis/launch-flags.md §2"]
tags: [exe, launch, flags]
updated: 2026-11-16
---
# Флаги запуска: места разбора

Адреса — виртуальные, издание PC 1.0.789.0. Флаги читаются из `cmd.txt` дважды: в состоянии `InitOnceMainLoop` и в настройке окна. Сравнение без учёта регистра, неизвестные слова пропускаются. Адрес в таблице — место разбора флага.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0067CD87` | `-stream ПАПКА ИМЯ…`: грузит `ПАПКА\ИМЯ.str`, переходит в `GameMainLoop`; работает с одним потоком | launch-flags.md §3 |
| `0x0067CD15` | `-playerspawn {GUID}`: GUID точки появления → `0x0112DACC`; работает | launch-flags.md §3 |
| `0x0067CCA1` | `-mainloop имя`: следующее состояние вместо `FrontendMainLoop`; жив, но бесполезен | launch-flags.md §3 |
| `0x0067CE60` | `-juice`: узнаётся и пропускается (заглушка) | launch-flags.md §3 |
| `0x0067CE87` | `-gameid строка`: строка в объекты сетевой игры; не проверялся | launch-flags.md §3 |
| `0x01223484`, `0x012234E4` | данные: указатели на объекты сетевой игры, куда пишется `-gameid` (`+0x10`) | launch-flags.md §3 |
| `0x0067CF15` | `-autoconnect`: результат сравнения не используется (мёртв) | launch-flags.md §3 |
| `0x0068D272` | `-fullscreen`: полный экран поверх `Window.Fullscreen` | launch-flags.md §3 |
| `0x0068D2A0` | `-windowed`: окно (проверено) | launch-flags.md §3 |
| `0x0068D2CD`, `0x0068D2FA` | `-vsync`, `-novsync`: поверх `Window.VSync`; не проверялись | launch-flags.md §3 |
| `0x0068D327` | `-resolution W H`: размер окна (проверено) | launch-flags.md §3 |
| `0x0068D395` | `-codepath число`: путь шейдеров (`0x0060F380` → `0x00F15A18`); не проверялся | launch-flags.md §3 |
| `0x0068D3C5` | `-persona имя`: копия в `0x00E50224`, читателей нет (мёртв) | launch-flags.md §3 |
| `0x00E50224` | данные: буфер имени `-persona` | launch-flags.md §3 |
| `0x0068D400` | `-inputwithoutfocus`: ввод без фокуса окна (`0x0040F9C0` → `0x0120524A`); не проверялся | launch-flags.md §3 |
| `0x00B3F108` | не флаг: байты команды `sub eax, 'ntfn'` (сравнение FourCC), ошибочно читаются как `-nftn` | launch-flags.md §3 |
| `0x00C7D40E` | не флаг: байты команды `sub eax, 'type'`, ошибочно читаются как `-epyt` | launch-flags.md §3 |
| `0x0068D25A` | второй разбор флагов — в настройке окна | launch-flags.md §2 |
| `0x0069BCC0`, `0x0069BC30` | чтение параметров `Window.Width`, `Window.Height`, `Window.Fullscreen`, `Window.VSync` и др. перед флагами | launch-flags.md §3 |
| `0x0069E3E0` | вызывается после разбора флагов окна | launch-flags.md §3 |

## Подробнее

- [Флаги запуска](../../analysis/launch-flags.md) §3
