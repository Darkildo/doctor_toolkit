---
type: exe
status: partial
confidence: exe
editions: ["1.0.789.0"]
stages: [75]
sources: ["docs/analysis/handing-out.md §2", "docs/analysis/handing-out.md §3", "docs/analysis/handing-out.md §4", "docs/analysis/handing-out.md §5", "research/exe_dis.py"]
tags: [exe, items, money, weapons, upgrades]
updated: 2026-11-16
---
# Выдача предметов, денег и улучшений: реле, подбор, адаптеры

Адреса — виртуальные, издание PC 1.0.789.0. Прочитаны дизассемблером `research/exe_dis.py` (Ghidra во время этапа 75 была недоступна): функции найдены по таблицам методов и местам вызова.

| Адрес | Что это | Источник |
|---|---|---|
| `0x00684D40` | фабрика класса `GiveItemRelay` (объект `0x8C` байт) | handing-out.md §2 |
| `0x0099E570` | конструктор `GiveItemRelay` | handing-out.md §2 |
| `0x00D927CC` | данные: таблица методов `GiveItemRelay` | handing-out.md §2 |
| `0x0099E610` | читатель атрибутов `GiveItemRelay`: 0 — сообщение, 1 — предмет, 2 — сколько, 3 — флаги | handing-out.md §2 |
| `0x0099E960` | выдача `GiveItemRelay`: поиск предмета того же вида, создание копий, взять в руку | handing-out.md §2 |
| `0x0099E790` | есть ли у цели предмет того же вида, что образец | handing-out.md §2 |
| `0x0099E810` | создать копию образца и отдать цели (`vtbl+0x248`) | handing-out.md §2 |
| `0x0099D2A0` | читатель атрибутов `AttributeRelay`: 0 — флаги, 1 — группа, 2 — GUID цели | handing-out.md §2 |
| `0x0099D380` | разбор цели `AttributeRelay`: объект из данных сообщения, объект или список по GUID, члены группы | handing-out.md §2 |
| `0x00707220` | вид предмета (`Item.3`) по образцу | handing-out.md §2 |
| `0x007040C0` | поиск предмета вида в инвентаре | handing-out.md §2 |
| `0x007078D0` | компонент объекта по хэшу интерфейса (инвентарь — `0x9A37DA3C`) | handing-out.md §2 |
| `0x00706C80` | читатель атрибутов `Item` (таблица переходов `0x00707198`) | handing-out.md §3.1 |
| `0x00704860` | вид предмета → слот инвентаря (таблица `0x007048E4`, 11 слотов) | handing-out.md §3.1 |
| `0x007A4520` | подбор предмета игроком (`vtbl+0x300`): аптечка, деньги, оружие | handing-out.md §3.2 |
| `0x00798810` | метод игрока `vtbl+0x248` «получить предмет» (→ `0x007A4520`) | handing-out.md §2 |
| `0x007062A0` | положить предмет в инвентарь по слоту вида | handing-out.md §3.2 |
| `0x007051D0` | слот огнестрельного оружия: замена оружием уровнем выше либо пополнение патронов | handing-out.md §3.2 |
| `0x00704920` | сравнение двух предметов одного слота по уровню (`+0x510`) | handing-out.md §3.2 |
| `0x00707ED0` | сообщение подбора `Item.14` с получателем в данных | handing-out.md §4.3 |
| `0x006865A0` | фабрика класса `MoneyAdaptor` (объект `0x64` байта) | handing-out.md §4.1 |
| `0x008A6D40` | конструктор `MoneyAdaptor` | handing-out.md §4.1 |
| `0x00D79B04` | данные: таблица методов `MoneyAdaptor` | handing-out.md §4.1 |
| `0x008A6DE0` | читатель атрибутов `MoneyAdaptor`: 0 — сумма, 1 — категория, 2 — звук | handing-out.md §4.1 |
| `0x008A14B0` | обработчик `EventAdaptor`: объект из данных сообщения → `Sentient` → метод адаптера | handing-out.md §4.1 |
| `0x008A6EE0` | действие `MoneyAdaptor`: семья персонажа, ограничение суммой книг, `AddMoney` | handing-out.md §4.1 |
| `0x008C29D0` | `AddMoney` с целой суммой (обёртка `0x008C09A0`) | handing-out.md §4.1 |
| `0x00892390` | сумма книг семьи по всем категориям | handing-out.md §4.2 |
| `0x00E547F8` | данные: таблица статистики паузы `{1, имя, функция}` (`MONEY_RACKET`…) | handing-out.md §4.2 |
| `0x009074E0` | статистика `MONEY_OTHER_INCOME`: книга категории 4 | handing-out.md §4.2 |
| `0x00907520` | статистика `MONEY_BRIBES_PAYOFFS`: книга категории 5 | handing-out.md §4.2 |
| `0x009075E0` | статистика `MONEY_LIFETIME_INCOME`: сумма книг семьи `corleone` | handing-out.md §4.2 |
| `0x009B8E80` | обработчик `TestUpgrade`: номер улучшения в списке семьи (`+0x124`, `+0x128`) | handing-out.md §5 |
| `0x009B9C60` | поиск `UpgradeData` по GUID в менеджере `*0x01129940` | handing-out.md §5 |
| `0x007A6390` | выдать улучшение игроку по номеру (сетевой запрос и `DonControlGadget`) | handing-out.md §5 |
| `0x0089B970` | `DonControlGadget`: улучшение из атрибута 5 — игроку | handing-out.md §5 |

## Что неизвестно

Выдача навыка `UpgradeData` в одиночной игре; что шлёт `Item.15`; действие `HonorAdaptor`, `ScoreAdaptor`, `StatAdaptor` — см. [вопрос](../questions/upgrade-grant-single-player.md).

## Подробнее

- [Выдача из логики](../../analysis/handing-out.md) §2–§5
- Связанные заметки: [семьи и деньги](family-money.md), [оружие](../systems/weapons.md), [экономика](../systems/economy.md)
