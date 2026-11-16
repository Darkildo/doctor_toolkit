---
type: question
status: unknown
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 47]
sources: ["docs/analysis/live-link.md §15.5"]
tags: [question, live-link, metamodel]
updated: 2026-11-16
---
# `Destructible` показывает «обломки» после скачка на метры за кадр

`Destructible` с состояниями по здоровью (metamodel самолёта: `health` 50–100 — целый, 0–49 — обломки) после сдвига вживую на метры за кадр показывает «обломки»; сдвиг на 0,05–0,3 м — цел. Причина не найдена: не физика тела и не включение экземпляра. В рабочем пространстве и после перезапуска объект целый.

## Следующий шаг

Найти, что меняет `health` или состояние metamodel при большом сдвиге (перехват записи), см. вопрос о состояниях metamodel.

## Связанное

- [Модуль в игре `doctor_live` (живая связь)](../engine/live-module.md)
- [Правка мира вживую: объект, коллизия, отмена (этап 47)](../experiments/live-world-editing.md)
- [Класс `Destructible`](../classes/Destructible.md)
- [Как движок выбирает начальное состояние metamodel и применяет переходы](metamodel-initial-state-and-transitions.md)
- [docs/analysis/live-link.md](../../analysis/live-link.md) §15.5
