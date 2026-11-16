---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 57]
sources: ["docs/analysis/game-bench.md §25"]
tags: [mcp, new-stream, collision, model-import, live-link]
updated: 2026-11-16
---
# Новая зона через MCP: стены с коллизией вокруг игрока (этап 57)

Зона собрана только инструментами MCP (`doctor-toolkit mcp ТЕНЬ --allow-edits --workspace W`),
рабочее пространство во временной папке.

| Шаг | Результат |
|---|---|
| `add_stream gf2/doctor/s57`, родитель `gf2/cuba/local/yuri/medium/zone4` | GUID `4324a758…`, хэш `002228f1`, записи в `cuba.toc` и `cuba_global.toc` |
| `add_simgroup` | пустая SimGroup `gf2/doctor/s57:simgroup:c29c48cb` |
| `import_model` стены 8 × 3 × 0,4 м (glTF с красной текстурой), образец — metamodel `a6379114`, `collision: box` | 9 ресурсов в новом потоке, `hkx` (`hkpBoxShape`), metamodel `75c04080` |
| 4 × `add_object`, 2 × `move_object` с поворотом 90° | стены по сторонам квадрата 8 × 8 м вокруг точки появления Q = (283,9; 0; 371,8) |
| `validate`, `mod_build` | `ok`, 10 изменений; мод формата 2, 1 новый поток |

В игре (`live install … --apply --workspace-dir W "--cmd-txt=-stream gf2 cuba"` — подмены этапа 46,
файлов в тени нет):

- `stream_load gf2/doctor/s57` — загружен за 70 кадров (повторно — за 10); `objects_near` у Q —
  4 `Destructible` нового потока ровно в 4 м; на снимке — красные стены вокруг игрока.
- Контроль (`stream_unload`, 4 раза `teleport` в Q и `hold w/s/a/d 6`): игрок уходит на 36,6 / 35,5 /
  34,4 / 36,0 м.
- Со стенами: игрок останавливается в 4,8 / 4,8 / 4,4 / 4,1 м от Q; |dx|, |dz| ≤ 3,41 м (внутренняя
  грань стены — 3,8 м, минус радиус капсулы).

Вывод: коллизия импортированной модели держит игрока; новая зона из нового потока грузится по
`stream_load` без записи в папку игры. 340 с без исключений; после `stop` и `live uninstall` —
`verify`: оригиналы не менялись (27 файлов), в тени нет `gf2/doctor/`.

Исправлено попутно: `validate` считал текстуры движка, которых нет ни в одном потоке, неразрешёнными
ссылками нового материала.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §25
- [MCP](../../mcp.md)
- [Руководство: MCP](../../guide/22-mcp.md)
- [Моды без записи в игру](mods-without-writing.md)
