---
type: question
status: partial
confidence: bench
editions: ["1.0.789.0"]
stages: [72, 74, 75]
sources: ["docs/scripts.md §9", "docs/analysis/lua-api.md §3", "docs/analysis/lua-api.md §4", "docs/analysis/game-bench.md §35"]
tags: [question, lua, simgroup]
updated: 2026-11-16
---
# Откуда скрипту Lua брать объекты игры (группы, функция `Base`)

Ответ этапа 74: в этой сборке — ниоткуда. `Base(строка)` (`0x46B7C0`) должна находить объект по тексту GUID, но разбор текста (`0x46B4D0`) сдвигает байт вместо слова, и из 32 цифр в поиск идут только байты 0, 4, 8 и 12 GUID, каждый один в своём слове. На стенде `Base` вернула пустую ссылку для 230 объектов 91 класса. Данные сообщений несут сырые указатели, а не `Base`. Группы (`GroupManager`, `Group`) работают только с `Base`; вызов `AddMember`, `RemoveMember`, `IsMember`, `RemoveFromAllGroups` с пустой ссылкой роняет игру (`0x00469F85`). `IsMember` и `Clear` в exe — заглушки. Пустая группа на стенде работает.

Догадка: объект мода с GUID вида `XX000000 YY000000 ZZ000000 WW000000` (байты файла) `Base` найдёт — тогда группы станут доступны для своих объектов.

Этап 75: догадка не проверялась (выдаче игроку она не нужна), но стала полезнее — предок реле [AttributeRelay](../classes/AttributeRelay.md) принимает имя группы (атрибут 1) и применяет действие к каждому её члену: скрипт с рабочими группами мог бы сам собирать цель выдачи.

## Следующий шаг

Создать в тени объект с таким GUID (нужна команда тулкита, задающая GUID нового объекта), вызвать `Base("{000000XX-00YY-0000-ZZ00-0000WW000000}")` на стенде и проверить `AddMember`, `GetMemberCount`, `ForEachMember`.

## Связанное

- [Lua в игре](../engine/lua.md)
- [Что лежит в данных сообщения (`data`) и как читать их из Lua](lua-message-data.md)
- [docs/scripts.md](../../scripts.md) §9
- [Lua-API движка](../../analysis/lua-api.md)
- [Функции и таблицы Lua из exe: полный список](../exe/lua-functions.md)
- [Lua-API на стенде: что проверено](../experiments/lua-api-on-bench.md)
- [Известные падения: адреса](../exe/known-crashes.md)
