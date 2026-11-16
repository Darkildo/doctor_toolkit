---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 17, 24, 45]
sources: ["docs/analysis/game-bench.md §13.6", "docs/analysis/game-bench.md §20.2"]
tags: [characters, partsdb, materials, viewer]
updated: 2026-11-16
---
# Одежда и цвета персонажей: правки баз partsdb в игре

Этап 24: тень с копией `allcharacters.str`, правки баз `partsdb` с `--direct`, сохранение на террасе.

| Правка | В игре |
|---|---|
| игрок, пресет `_all_main` / `a_default`: замена `char_tmp_checker_mat01` (`98199de4`) → `yellow_leisure_ls01_tkcl01_gen_gen01_gen01_ply01_na` вместо `leisure_ls01_tkcl01_burgundy` | видно: рубашка светло-жёлтая вместо бордовой, рукава прежние (у них своя замена) |
| Майкл (`unq_michael_corleone`, пресет `unq_0001_main` набора `unq_0001_miami`): `jacket_classic_op01_mat` → `jacket_classic_op01_ivory` | видно: пиджак цвета слоновой кости |
| игрок: `partsdb allow … player a_hat_fedora --set _all --only` | через сохранение не видно: выбор слотов игрока хранится в сохранении, база даёт только варианты |

- Хэш замены — CRC-32 имени материала.
- Сравнение с просмотрщиком (`DOCTOR_TOOLKIT_GUI_CHARACTER`, lavapipe): оттенки рубашки, пиджака, брюк
  и кожи совпадают; в игре сцена темнее и с зелёным светом фейерверка.
- Набор `_all` у Майкла без пресета даёт в просмотрщике служебные материалы-шаблоны — пресет есть
  только в `unq_0001_miami`.

**Этап 45:** при старте `-stream` (сохранённого выбора слотов нет) на игроке видны и шляпа
`a_hat_fedora` (`partsdb allow --only`), и новая часть `doctor_jacket` (`model-import --new` +
`partsdb add-part --slot a_necklace --set _all --only`).

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §13.6, §20.2
- [Формат персонажей](../../formats/characters.md)
- [Новая одежда со скиннингом](new-clothes-skinning.md)
