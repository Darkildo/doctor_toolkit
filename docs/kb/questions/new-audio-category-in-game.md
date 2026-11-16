---
type: question
status: hypothesis
confidence: exe
editions: ["1.0.789.0"]
stages: [83]
sources: ["docs/formats/audio.md §13", "docs/formats/audio.md §14"]
tags: [question, audio, mixer, bench]
updated: 2026-11-16
---
# Новая категория звука и правка микса в игре не проверены

Тулкит добавляет категорию в `auc` и меняет каналы `mix`, ресурсы после правки читаются обратно. По коду категория регистрируется по хэшу имени при загрузке `auc` (`0x005DC510`), так что новая должна появиться, — в игре это не проверено.

## Что не проверено

- принимает ли игра 50-ю категорию (пулы и таблицы микшера могут иметь предел);
- слышна ли правка громкости категории в миксе;
- находит ли дак `CHATTER_FOLLOWUP_DUCK` свой микс: имя микса в `auc` длиннее 31 символа, а в ресурсе `mix` обрезано.

## Следующий шаг

Стенд с записью звука: `mix global_default.mix --category MUSIC --volume 0` и музыка в меню; затем категория `DOCTOR_VOICE` под `DIALOG`, реплика с `--category DOCTOR_VOICE` и канал этой категории в `global_default.mix` с громкостью 0 и 1.

## Связанное

- [Тип ресурса `auc`](../resources/auc.md)
- [Тип ресурса `mix`](../resources/mix.md)
- [Формат `mix`: смысл уровней](resource-format-mix.md)
