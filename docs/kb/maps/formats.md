---
type: map
status: known
confidence: data
editions: ["1.0.789.0"]
stages: [72]
sources: []
tags: [map, formats]
updated: 2026-11-16
---
# Карта: форматы

Форматы файлов игры. Заметка на каждый тип ресурса — в [списке типов ресурсов](../resources/index.md); подробные описания — в документах `docs/formats/` ([оглавление](../../formats/README.md)). Назад: [карта базы](../index.md).

## Контейнеры

- Мировая сетка `wgi` — [формат](../../formats/wgi.md): ячейки зон, таблица потоков, как игра выбирает зоны; заметка [типа](../resources/wgi.md), [решённый вопрос](../questions/wgi-format-and-zone-stream-enable.md).
- Поток `.str` — [формат потока](../../formats/stream.md); таблица содержимого — [TOC](../../formats/toc.md); сжатие и хэши — [RefPack и SDBM](../../formats/compression-and-hash.md). Заметки: [потоки и их загрузка](../engine/stream-loading.md), [хэши](../engine/hashing.md).
- Новые ресурсы и потоки — [анализ](../../analysis/new-resources.md); сверка форматов с файлами игры — [протокол](../../analysis/gf2-pc-verification.md).

## Картинка

- Текстуры [`tg4h`](../resources/tg4h.md) и [`tg4d`](../resources/tg4d.md), интерфейс [`apx`](../resources/apx.md), шрифты [`ffn`](../resources/ffn.md), строки [`lh2`](../resources/lh2.md) и [`lhr`](../resources/lhr.md) — [каталог типов](../../formats/resource-types.md).
- Геометрия [`mesh`](../resources/mesh.md), [`meshvolatile`](../resources/meshvolatile.md), материалы [`material`](../resources/material.md) — [геометрия](../../formats/geometry.md); деревья [`treemodel`](../resources/treemodel.md) — [модели деревьев](../../formats/treemodel.md).
- Персонажи: [`partsdb`](../resources/partsdb.md), [`mobface`](../resources/mobface.md) — [персонажи](../../formats/characters.md).
- Видео `.vp6` — [видео](../../formats/video.md).

## Движение и физика

- Риги [`rcb`](../resources/rcb.md) — [риги](../../formats/rig.md); анимации [`bnk`](../resources/bnk.md) — [граф анимации](../../formats/anim-graph.md).
- Коллизии [`hkx`](../resources/hkx.md) — [Havok](../../formats/havok.md).
- Навигация [`graph`](../resources/graph.md) — [навигационный граф](../../formats/navigation-graph.md); точки [`covernodetable`](../resources/covernodetable.md).

## Логика и текст

- [`simgroup`](../resources/simgroup.md) — [SimGroup](../../formats/simgroup.md); [`metamodel`](../resources/metamodel.md) — [MetaModel](../../formats/metamodel.md); [`variabledictionary`](../resources/variabledictionary.md) — [типы логики](../../formats/logic-types.md).
- Скрипты [`lua`](../resources/lua.md) — [Lua](../../formats/lua.md); логика миссий — [где живёт логика](../../formats/mission-logic.md).
- Диалоги [`dlg`](../resources/dlg.md), речь [`chv`](../resources/chv.md), [`smb`](../resources/smb.md) — [диалоги](../../formats/dialogues.md).
- Ролики [`trinity_seq_master`](../resources/trinity_seq_master.md), субтитры [`tob`](../resources/tob.md) — [ролики](../../formats/cutscenes.md).

## Звук

- Банки [`sbk`](../resources/sbk.md), архивы `.viv` — [звук](../../formats/audio.md).

## Не разобрано

Типы без разбора — отдельными вопросами: [`score`](../questions/resource-format-score.md), [`ism`](../questions/resource-format-ism.md), [`mnl`](../questions/resource-format-mnl.md), [`vfx`](../questions/resource-format-vfx.md) и другие — см. [список вопросов](../questions/index.md).
