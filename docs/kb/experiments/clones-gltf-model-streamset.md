---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 5, 10, 24]
sources: ["docs/analysis/game-bench.md §13.5", "docs/analysis/game-bench.md §13.13"]
tags: [clone, gltf, streams, streamset, model-import]
updated: 2026-11-16
---
# Клоны, новая модель из glTF и поток через StreamSet видны в игре

Этап 24, прогон `rD` (тень с копиями зоны, `gf2/cuba.str`, `gf2/globals_global.str`), `TRACE_FILES=1`.
Набор тележки — metamodel `daa0769a` (№ 88), два меша, шесть материалов, четыре текстуры, дочерний
metamodel тарелок.

| Правка | Результат |
|---|---|
| клон набора в том же потоке (`clone-resource … --seed doctor-red`), три текстуры клона красные, тележка 97 переведена на клон (`simgroup-set … 97 --model 5528b99c`) | видно: тележка красная |
| новая модель из glTF: меш тележки ×1,3 по X, ×2,2 по Y как новый набор `doctor_tall_cart` (`model-import … --new … --retarget <SimGroup>`) | видно: высокая узкая тележка, материалы прежние |
| поток по запросу `gf2/doctor/s24set` (родитель `gf2/cuba`, флаги `0,0,1,0`) со своим SimGroup и жёлтым набором; на поток ссылается только `StreamSet` № 49 группы `gf2/cuba` (атрибут 5 = GUID) | загружается и виден: `s24set.str` открыт (раньше зоны), жёлтая тележка над перилами |
| дочерний поток `gf2/doctor/s24child` (`--with-parent`, флаги `0,0,5,0`) | не загружается — см. [дочерний поток](child-stream-not-opened.md) |

Что из этого следует:

- Для своих ресурсов в уровне надёжен поток по запросу через `StreamSet` или вставка в существующий
  поток.
- В клон для потока, который не наследует зону, должны входить и коллизия (`hkx` 65, 67), и `vfx` 75 —
  иначе ссылки клона не разрешаются (`check-refs`).

Найдено и исправлено попутно: `model-import --new` в потоке с применёнными клонами отказывал
(«… is not a resource of the original …»); `texture-import` не мог перекрасить клон текстуры с тем же
именем — теперь принимается `ИМЯ_ХЭШ`.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §13.5, §13.13
- [Новые ресурсы и потоки](../../analysis/new-resources.md)
- [Импорт моделей](../../model-import.md)
