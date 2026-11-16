---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 48]
sources: ["docs/analysis/live-link.md §13.2", "docs/analysis/live-link.md §13.9", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs"]
tags: [exe, weather, world]
updated: 2026-11-16
---
# Погода: WeatherManager (дождь)

Адреса — виртуальные, издание PC 1.0.789.0. Класс `WeatherManager` — это дождь: случайный по таймеру, с плавным началом и концом. Ни одна SimGroup игры такой объект не создаёт, на стенде указатель пуст.

| Адрес | Что это | Источник |
|---|---|---|
| `0x0112AFF4` | данные: объект `WeatherManager` (указатель); флаги `+0x84`, длина и время дождя `+0xB0`, `+0xB4`; на стенде 0 | live-link.md §13.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00836CC0` | конструктор `WeatherManager` | live-link.md §13.2 |
| `0x00836CD5` | место конструктора, где объект кладётся в `*0x0112AFF4` | crates/doctor-toolkit-live/src/addr.rs; crates/doctor-toolkit-live/src/edition.rs |
| `0x00837940` | случайный дождь по таймеру (вероятность — атрибут 11) | live-link.md §13.2 |
| `0x008371B0` | под крышей — без капель | live-link.md §13.2 |
| `0x008369D0` | регистрация сообщений управления из атрибутов 0–5 | live-link.md §13.2 |
| `0x00837730` | начать дождь с переходом | live-link.md §13.2 |
| `0x00836820` | закончить дождь | live-link.md §13.2 |
| `0x00837860` | начать дождь сразу | live-link.md §13.2 |
| `0x008368B0` | закончить дождь сразу | live-link.md §13.2 |

## Подробнее

- [Живая связь](../../analysis/live-link.md) §13.2 «Погода»
