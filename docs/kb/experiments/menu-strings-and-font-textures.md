---
type: experiment
status: known
confidence: bench
editions: ["1.0.789.0"]
stages: [72]
sources: ["docs/analysis/game-bench.md §7"]
tags: [text, textures, frontend, menu]
updated: 2026-11-16
---
# Правленые строки и атласы шрифтов видны в меню

Первые проверки стенда (2026-11-12): правки в тени, прогон до меню «Select Profile».

- **Строки.** `text-import` в `gf2/text/dd7d7050.str` (английская таблица `lh2`): изменены
  `Press any key` (хэш `cbd0a3c6`), `Select Profile` (`486f5259`), `Create Profile` (`11132259`);
  тулкит обновил `gf2/frontend.toc`, `frontend_global.toc`, `globals_global.toc`. На экране —
  «Press any key - doctor_toolkit», «TOOLKIT PROFILES», «Make Toolkit Profile».
- **Текстуры.** `texture-import` PNG: атласы шрифтов `impact_lt_std` (512×512) и `aachen_std_bold`
  (1024×512) в `gf2/frontend.str`, альфа-канал инвертирован, цвет изменён; формат DXT5 и полная
  цепочка мипов сохранены, TOC не менялся. Все надписи меню выводятся «негативом».

Что из этого следует:

- Игра берёт из атласа шрифта только альфа-канал; цвет текста задаёт сама.
- В `gf2/frontend.str` всего три текстуры `tg4h`/`tg4d`: два атласа шрифтов и `buttons`. Фон меню и
  логотип — во Flash-интерфейсах `apx` и видеороликах.
- Хэши оригиналов до и после совпали (30 файлов), в каталоге игры нет файлов новее начала опытов.

## Подробнее

- [Стенд проверки в игре](../../analysis/game-bench.md) §7
- [Текстуры APX и SimGroup в меню](apx-textures-and-simgroup-in-menu.md)
