---
type: question
status: unknown
confidence: exe
editions: ["1.0.789.0"]
stages: [72, 75]
sources: ["docs/formats/dialogues.md §8"]
tags: [question, dialogues, f2f]
updated: 2026-11-16
---
# Названия «переменных» условий диалогов и слов 1–4 выбора

Условия узлов диалога ссылаются на «переменные» — номера запросов функции `0x006EBFF0`; их названия не известны. Не названы и слова 1–4 узла выбора F2F (`0x22`).

## Следующий шаг

Разобрать таблицу запросов `0x006EBFF0` в Ghidra; оценить, годится ли узел выбора как меню (пример «торговец»).

Запланировано: [Этап 75: Предметы, деньги, оружие из логики; выбор как меню](../research/stage-75.md).

## Связанное

- [Диалоги F2F и речь](../systems/dialogues.md)
- [Диалоги: загрузчики dlg и chv, узлы](../exe/dialogues.md)
- [Тип ресурса `dlg`](../resources/dlg.md)
- [docs/formats/dialogues.md](../../formats/dialogues.md) §8
