---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 18]
sources: ["docs/analysis/game-bench.md §11"]
tags: [text, apx, frontend]
updated: 2026-11-16
---
# Новая строка находится игрой по имени из интерфейса

Проверка этапа 18, прогон A (до меню).

- `text-add … doctor_title "DOCTOR TOOLKIT: NEW STRING" --lang es=…` — запись `3b71ce38` во все
  четыре таблицы языков.
- `apx-text-refs … gf2/frontend 20 --set fe_press_start=doctor_title` — ссылка интерфейса на новую
  строку.

Результат: на титульном экране вместо «Press any key» — «DOCTOR TOOLKIT: NEW STRING». Новая запись
найдена игрой по имени из интерфейса.

Тот же приём использован в проверке «Играть» с записью и откатом (строка `doctor_play`) —
[запись и откат](play-apply-revert-on-bench.md).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §11 (проверки этапа 18)
