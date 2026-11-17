---
type: question
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 74]
sources: ["docs/scripts.md §9", "docs/analysis/lua-api.md §5", "docs/analysis/game-bench.md §35"]
tags: [question, lua, audio]
updated: 2026-11-16
---
# `PlayEarsAudioStream` из своего скрипта не проверен в игре

Решено этапом 74. `PlayEarsAudioStream(имяИлиHandle [, громкость = 1 [, категорияМикса]])` (`0x438D10`): имя ищется как есть, поэтому нужно с `.stx`; результат — light userdata со счётчиком запусков; неизвестное имя даёт light userdata 0 без ошибки (в Lua это истинное значение). На стенде поток из своего скрипта слышен в записи звука, `PrimeEarsAudioStream` готовит поток молча, `StopEarsAudioStream` его гасит, `SetEarsAudioStreamStopMessage` шлёт сообщение после остановки. Узел «Play stream» дописывает `.stx` сам.

Осталось мелкое: поток `accomplishment_01.stx` за 70 с сам не кончился, поэтому стоповое сообщение при естественном конце не наблюдалось; как аргумент громкости переводится в громкость, по записи не видно.

## Следующий шаг

Проиграть на стенде короткий поток до конца и проверить стоповое сообщение; сравнить громкости в тихом месте уровня.

## Связанное

- [Lua в игре](../engine/lua.md)
- [Звук](../engine/audio.md)
- [Свои скрипты Lua: где они запускаются и что нужно для этого](../experiments/lua-own-scripts.md)
- [docs/scripts.md](../../scripts.md) §9
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
