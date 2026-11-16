---
type: question
status: partial
confidence: data
editions: ["1.0.789.0"]
stages: [72, 73, 98]
sources: ["docs/formats/resource-types.md §8", "data/gf2/enums.toml", "research/enum_values.py"]
tags: [question, resource-type, format]
updated: 2026-11-16
---
# Формат ресурса `pad` не разобран

Тип `pad`: эмоции пешеходов (`Neutral`, `Angry`), два ресурса по 3 952 байта. Разбора в тулките нет.

Этап 73 прочитал из него перечисление [`emotion`](../enums/emotion.md): `u32` число записей (7), `u32` `sdbm(lower(имя набора))`, с `0x18` имя набора (`pedestrian`), с `0x3C` — 7 записей по 556 байт: `u32` (у всех 128), `u32` `sdbm(lower(имя))`, имя (`Neutral`, `Angry`, `Disgusted`, `Fearful`, `Happy`, `Sad`, `Surprised`), дальше числа `f32`. Смысл чисел не разобран; два ресурса отличаются значениями. На ресурс ссылается `Sentient` 5 у всех 3 433 персонажей. Название типа, вероятно, от модели эмоций «pleasure — arousal — dominance» — догадка.

## Следующий шаг

Выгрузить ресурсы (`doctor-toolkit dump <игра> --type pad`), описать раскладку и найти загрузчик в exe.

Запланировано: [Этап 98: Неопознанные типы ресурсов](../research/stage-98.md).

## Связанное

- [Тип ресурса `pad`](../resources/pad.md)
- [docs/formats/resource-types.md](../../formats/resource-types.md) §8
