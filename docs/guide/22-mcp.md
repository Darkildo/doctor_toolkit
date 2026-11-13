# 22. ИИ-помощник: игра через MCP

ИИ-помощник вроде Claude Code или Claude Desktop может сам смотреть файлы
игры: искать ресурсы, читать объекты уровней и миссий, идти по сообщениям
логики, находить строки и имена хэшей. Для этого у тулкита есть сервер
**MCP** (Model Context Protocol) — команда `doctor-toolkit mcp`. Помощник
запускает её сам и задаёт вопросы через неё.

Сервер **только читает**: он ничего не пишет ни в игру, ни в рабочее
пространство, ни в проект. Правки помощник пока делать не может — это
следующий этап.

Устройство и полный список инструментов — `docs/mcp.md`.

## 22.1 Что понадобится

- программа `doctor-toolkit` из выпуска (глава 1) или собранная из
  исходников; полный путь к ней, например
  `/home/me/doctor_toolkit-0.1.0-linux-x86_64/doctor-toolkit`;
- папка игры (глава 2), например
  `/home/me/.local/share/Steam/steamapps/common/The Godfather II`.

Проверьте, что сервер запускается: он напишет одну строку в stderr и будет
ждать клиента. Закройте его Ctrl+D.

```sh
doctor-toolkit mcp "/путь/к/The Godfather II"
```

Индекс игры берётся из кэша, как у редактора; если его ещё нет, первый
вопрос помощника займёт около минуты.

## 22.2 Claude Code

Одна команда добавляет сервер (для текущего проекта; `--scope user` — для
всех):

```sh
claude mcp add doctor-toolkit -- doctor-toolkit mcp "/путь/к/The Godfather II"
```

Если `doctor-toolkit` не в `PATH`, укажите полный путь к программе. С
рабочим пространством в другой папке или с проектом мода:

```sh
claude mcp add doctor-toolkit -- doctor-toolkit mcp "/путь/к/игре" \
    --workspace /путь/к/workspace --project ~/mods/my-mod
```

Проверка: `claude mcp list` показывает `doctor-toolkit … ✓ Connected`, а в
сессии команда `/mcp` — сервер и его 12 инструментов. Удалить:
`claude mcp remove doctor-toolkit`.

Тот же сервер можно описать файлом `.mcp.json` в папке проекта (его
читает Claude Code):

```json
{
  "mcpServers": {
    "doctor-toolkit": {
      "command": "doctor-toolkit",
      "args": ["mcp", "/путь/к/The Godfather II"]
    }
  }
}
```

## 22.3 Claude Desktop

Откройте настройки → Developer → Edit Config (файл
`claude_desktop_config.json`: в Linux `~/.config/Claude/`, в macOS
`~/Library/Application Support/Claude/`, в Windows `%APPDATA%\Claude\`) и
добавьте сервер:

```json
{
  "mcpServers": {
    "doctor-toolkit": {
      "command": "/home/me/doctor_toolkit-0.1.0-linux-x86_64/doctor-toolkit",
      "args": ["mcp", "/home/me/.local/share/Steam/steamapps/common/The Godfather II"]
    }
  }
}
```

Путь к программе здесь нужен полный: Claude Desktop не видит `PATH`
терминала. Перезапустите Claude Desktop; в окне сообщения появится значок
инструментов с `doctor-toolkit`.

## 22.4 Что спросить

- «Что за игра подключена? Сколько в ней потоков и ресурсов?» — `game_info`.
- «Найди все SimGroup района Бонита», «где используется материал
  `brick_wall`?» — `search`, `resource_info`.
- «Покажи объекты уровня `gf2/miami/local/bonita/high/terrain010` рядом с
  точкой (−350, −20, 640)» — `level_objects`.
- «Кто слушает сообщение `iMsgNewCheckpointLoaded`? Что происходит дальше
  на два шага?» — `logic_graph`.
- «Чем заканчивается миссия `cp_010_030_mission_1`? Какие сообщения в ней
  никто не слышит?» — `logic_graph` со `scope`.
- «Найди реплики, где упоминается Фредо», «что за хэш `38523fc3`?» —
  `strings`, `lookup_hash`.
- «Покажи диалог `0823c6d1` целиком», «декомпилируй этот скрипт Lua» —
  `resource_json`.
- «Что я поменял в рабочем пространстве и что ещё не записано в игру?» —
  `workspace_status`, `workspace_diff`; с проектом — `project_status`.

Помощник также может прочитать описания форматов — ресурсы
`doctor-toolkit://docs/formats/*.md` (в Claude Code — упоминанием `@` в
сообщении).

## 22.5 Безопасность

- Сервер работает на вашем компьютере, по stdio, без сети.
- Он не принимает путей к файлам: только имена потоков и ключи ресурсов
  игры. Читает папку игры, рабочее пространство и проект, которые вы
  указали при запуске.
- Тексты игры и модов приходят помощнику огороженными как данные
  (`<game-data>`), с предупреждением не выполнять инструкции из них.
- Большие ответы приходят частями; предел одного ответа —
  `--max-response-chars` (40 000 символов по умолчанию).
- Единственное, что сервер может записать, — кэш индекса в
  `~/.cache/doctor-toolkit/`, как и редактор; `--no-cache` отключает и его.

## 22.6 Неполадки

| Что видно | Что сделать |
|---|---|
| `Failed to connect` в `claude mcp list` | запустите ту же команду в терминале: ошибка (нет папки игры, это не игра) будет в stderr |
| первый ответ идёт минуту | строится индекс игры; дальше он берётся из кэша |
| помощник не видит свежих правок игры после `workspace apply` | индекс читается при запуске сервера: перезапустите его (`/mcp` → reconnect в Claude Code) |
| ответ обрезан | попросите следующую страницу (`offset`) или часть (`pointer`), либо увеличьте `--max-response-chars` (Claude Code по умолчанию принимает до 25 000 токенов) |
