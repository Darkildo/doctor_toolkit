---
type: exe
status: known
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 47, 34]
sources: ["docs/analysis/live-link.md §15.1", "docs/analysis/live-link.md §15.2", "docs/analysis/live-link.md §3", "crates/doctor-toolkit-live/src/addr.rs", "crates/doctor-toolkit-live/src/edition.rs", "godfather2.exe 0x006101B0"]
tags: [exe, entity, world, matrix]
updated: 2026-11-16
---
# Entity: матрица, видимость, экземпляр модели

Адреса — виртуальные, издание PC 1.0.789.0. У `Entity` узел преобразования лежит в `+0x90`: мировая матрица — `+0x90`, локальная — `+0xD0`, родитель — `+0x110`; без родителя они равны, и модуль читает `+0xD0`. Штатный `SetMatrix` двигает модель, сообщает рендеру и пространственной части. Модуль правки мира двигает только объекты, у которых слот `+0x28` — ровно `Entity::SetMatrix`.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00E32858` | данные: таблица методов `Entity` | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00E32880` | данные: слот `+0x28` таблицы `Entity` — `SetMatrix` | crates/doctor-toolkit-live/src/edition.rs |
| `0x00D7B5D8` | данные: таблица методов `Destructible` (слоты матрицы те же) | live-link.md §15.1 |
| `0x0046E6B0` | конструктор `Entity`: пишет таблицу методов, заводит узел преобразования `+0x90` (`0x0060FFD0`: единичные матрицы в `+0x90` и `+0xD0`, родителя нет) | live-link.md §3; live-link.md §15.1 |
| `0x00471610` | доступ к локальной матрице: `lea eax,[ecx+0xD0]` | live-link.md §3; crates/doctor-toolkit-live/src/edition.rs |
| `0x00470E30` | `Entity::SetMatrix(this, const Matrix*)`, `thiscall`, `RET 4`: масштаб в `+0x134..+0x13C`, матрица в локальную `+0xD0`, затем `0x006101B0` и виртуальный `+0x68` | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x00470E6C` | место `SetMatrix`: масштаб `+0x134` | crates/doctor-toolkit-live/src/edition.rs |
| `0x00470ED2` | место `SetMatrix`: узел `+0x90`, локальная матрица `+0xD0` | crates/doctor-toolkit-live/src/edition.rs |
| `0x00470F2A` | место `SetMatrix`: мировая матрица узла `0x006101B0`, виртуальный `+0x68`, `RET 4` | crates/doctor-toolkit-live/src/edition.rs |
| `0x006101B0` | пересчёт мировой матрицы узла (`ECX` = объект + `0x90`): без родителя (`узел+0x80`) копирует локальную `узел+0x40` в мировую `узел+0`, с родителем умножает (`0x00417560`); затем обход детей (`узел+0x84`, братья `+0x88`) | live-link.md §15.1 |
| `0x004703C0` | виртуальный `+0x68` «матрица изменилась»: экземпляр модели в список сдвинутых, команда рендеру `0x0110BA6C`, пространственной части — `+0x28(2)` | live-link.md §15.1 |
| `0x01205488`, `0x0120548C` | данные: список сдвинутых экземпляров моделей | live-link.md §15.1 |
| `0x00442B70` | разбирает список сдвинутых моделей (зовёт виртуальный `+0x30` экземпляра) | live-link.md §15.1 |
| `0x004A47E0` | виртуальный `+0x30` экземпляра модели: двигает части | live-link.md §15.1 |
| `0x0110BA6C` | данные: команда рендеру «новая матрица объекта» (пишется в буфер кадра) | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x0110BA94` | данные: команда рендеру «показать/скрыть экземпляр» | live-link.md §15.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x00E35378` | данные: таблица методов экземпляра metamodel (0xA0 байт) | live-link.md §15.1 |
| `0x004A4960` | экземпляр модели: включение (`+0x24`) | live-link.md §15.1 |
| `0x004A4B10` | экземпляр модели: выключение (`+0x28`); части `+0x68` по 0x10 байт, число `+0x6C` | live-link.md §15.1; crates/doctor-toolkit-live/src/addr.rs |
| `0x004A4B2B` | место цикла выключения частей экземпляра (сверяется при проверке издания) | live-link.md §15.1; crates/doctor-toolkit-live/src/edition.rs |
| `0x0046EDB0` | `CMD_SetModel` (→ `0x00460C20`): ставит экземпляр metamodel в `+0x130` | live-link.md §15.1 |
| `0x00460C20` | создание экземпляра модели из `CMD_SetModel` | live-link.md §15.1 |
| `0x00471DB0` | `Entity::SetVisible(this, bool)`, `thiscall`, `RET 4`: команда рендеру `0x0110BA94` | live-link.md §15.2; crates/doctor-toolkit-live/src/addr.rs |
| `0x008B9020`, `0x008B8FE0` | скрытие и показ разрушаемых объектов (зовут `SetVisible`, когда объект остаётся активным) | live-link.md §15.2 |
| `0x00470460` | счётчик выключений `+0x161`: +1, с нуля — виртуальный `+0x130` | live-link.md §15.2 |
| `0x00470490` | счётчик выключений `+0x161`: −1, до нуля — виртуальный `+0x12C` | live-link.md §15.2 |
| `0x00471EB0` | виртуальный `+0x130`: вынуть объект из мира (экземпляр `+0x28` и команда рендеру «скрыть») | live-link.md §15.2 |
| `0x00471DF0` | виртуальный `+0x12C`: вернуть объект в мир | live-link.md §15.2 |
| `0x004A6FA0`, `0x004A6F30` | остановка и запуск машины состояний metamodel | live-link.md §15.1 |

## Что неизвестно

Статическая коллизия за новой матрицей сама не идёт (см. заметку о частях коллизии).

## Подробнее

- [Живая связь](../../analysis/live-link.md) §15.1, §15.2
