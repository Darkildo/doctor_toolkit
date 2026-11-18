gf2-unify — единая игра The Godfather II (PC)
=============================================

Собирает новую папку игры — версия 1.0.789.0, языки: английский, испанский,
русский (текст и озвучка), чешский, нидерландский, польский (текст) — из вашей
лицензионной копии (диск или установленная игра, издание PAL 1.0.764.0 или
NTSC 1.0.766.0) и пакета с тем, чего вашему изданию не хватает. Ваша копия
только читается. Без своей копии игры программа ничего не даёт.

Как пользоваться
1. Запустите gf2-unify (двойной щелчок). Откроется окно.
2. Пакет раздаётся торрентом: в окне есть ссылка и кнопка «Открыть в
   торрент-клиенте». В раздаче отметьте папку common и часть своего издания
   (for-pal или for-ntsc; если не уверены — обе) и скачайте рядом с
   программой, чтобы рядом с ней появилась папка pack.
3. Укажите вашу копию игры (диск — это смонтированный образ или сам диск) и
   новую папку для игры (нужно около 9,3 ГБ), выберите язык и нажмите
   «Собрать игру».
4. Игра запускается файлом godfather2.exe из новой папки.

Окно само говорит, какое у вас издание, все ли нужные файлы пакета скачаны и
тот ли это пакет, что опубликован с программой. Каждый файл сверяется по
SHA-256; изменённая копия (моды, репак) чинится файлами пакета, когда они в
нём есть.

Язык потом меняется командой: gf2-unify language ru   (en, es, ru, cs, nl, pl)

Подробно: docs/unified-game.md в репозитории
https://github.com/Darkildo/doctor_toolkit


gf2-unify — the unified game of The Godfather II (PC)
=====================================================

Makes a new game folder — version 1.0.789.0 with English, Spanish, Russian
(text and speech), Czech, Dutch and Polish (text) — from your own licensed
copy (a disc or an installed game, PAL 1.0.764.0 or NTSC 1.0.766.0) and a pack
with what your edition lacks. Your copy is only read. Without a copy of the
game the program gives nothing.

How to use
1. Start gf2-unify (a double click). A window opens.
2. The pack is shared by torrent: the window shows the link and the button
   "Open in the torrent client". Tick the folder common and the part of your
   edition (for-pal or for-ntsc; both when not sure) and download next to the
   program, so that the folder pack lies beside it.
3. Choose your copy of the game (a disc is a mounted image or the disc
   itself) and a new folder for the game (about 9.3 GB), pick the language and
   press "Make the game".
4. Start the game with godfather2.exe of the new folder.

The window tells which edition you have, whether every needed file of the pack
is downloaded and whether the pack is the one published with the program.
Every file is checked by SHA-256; a changed copy (mods, a repack) is repaired
with files of the pack when the pack has them.

The language is changed later with: gf2-unify language ru   (en, es, ru, cs, nl, pl)

Command line: gf2-unify --help
