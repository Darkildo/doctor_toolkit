# 1. Установка

## 1.1 Готовый архив для Linux

Релиз — архив `doctor_toolkit-<версия>-linux-x86_64.tar.gz` и файл с его
SHA-256 рядом.

```sh
sha256sum -c doctor_toolkit-0.0.1-linux-x86_64.tar.gz.sha256
tar xzf doctor_toolkit-0.0.1-linux-x86_64.tar.gz
cd doctor_toolkit-0.0.1-linux-x86_64
./doctor-toolkit --version
./doctor-toolkit-gui
```

В архиве:

| Файл | Что это |
|---|---|
| `doctor-toolkit` | командная строка |
| `doctor-toolkit-gui` | редактор |
| `doctor-toolkit-mods` | менеджер модов для игроков |
| `README.md`, `LICENSE` | краткое описание на английском, лицензия GPL-3.0 |
| `docs/` | это руководство и описания форматов |
| `BUILD-INFO.txt` | версия, коммит, компилятор, минимальная версия glibc, версии символов ALSA, нужные редактору |
| `install-desktop.sh`, `desktop/`, `icons/` | ярлыки редактора и менеджера модов для меню приложений и их значки |

Установка не нужна: программы можно запускать из любого каталога. Чтобы
вызывать их по имени, скопируйте их в `~/.local/bin` (он обычно уже в `PATH`).
Чтобы они появились в меню приложений со значками, запустите
`./install-desktop.sh` (убрать — `./install-desktop.sh --uninstall`; раздел
[10.7](10-interface.md#107-ярлык-в-меню-приложений)).

## 1.2 Требования

| Что | Требование |
|---|---|
| Система | Linux x86_64 с glibc версии не ниже указанной в `BUILD-INFO.txt` (архивы релизов собираются под glibc 2.30: Ubuntu 20.04, Debian 11, Fedora 31 и новее) |
| Библиотеки при запуске | `libc`, `libm`, `libgcc_s` — есть в любой системе; редактору (`-gui`) ещё `libasound.so.2` (ALSA) — для звука |
| Звук (`-gui`) | `libasound.so.2`: пакет `libasound2` (Ubuntu 24.04 и новее — `libasound2t64`), Fedora `alsa-lib`, Arch `alsa-lib`; в настольных системах уже установлен (его тянут PipeWire, PulseAudio, браузеры). Без него редактор не запускается: `error while loading shared libraries: libasound.so.2` — поставьте пакет. Звуковая карта не обязательна: без устройства вывода звук молчит, ролики и звуки идут по таймеру |
| Окно (`-gui`, `-mods`) | X11 (`libX11`, `libxkbcommon-x11`, `libXcursor`, `libXi`) или Wayland (`libwayland-client`, `libxkbcommon`); загружаются при запуске окна |
| Графика | Vulkan (`libvulkan.so.1` и драйвер Mesa/NVIDIA) или OpenGL 3.3 (`libGL`/`libEGL`). Без Vulkan редактор сам переходит на OpenGL; модели тогда показываются программным просмотрщиком |
| Диалоги выбора файлов | портал `xdg-desktop-portal` (есть в GNOME, KDE и большинстве окружений), иначе программа `zenity`. Без них каталог игры можно передать аргументом: `doctor-toolkit-gui "/путь/к/игре"` |
| Место на диске | запись потока требует места под его полную копию; резервные копии — столько, сколько весят изменённые файлы (до 140 МБ на поток) |

Командной строке графика и звук не нужны: она работает и на сервере без
экрана. Менеджеру модов (`-mods`) звук тоже не нужен.

Звук редактора (звуковые банки, ролики, Movies) играет на устройство вывода
по умолчанию через ALSA; PipeWire и PulseAudio подхватывают его через свой
модуль ALSA (`pipewire-alsa`, `alsa-plugins-pulseaudio`). Если устройства
нет или оно занято, редактор пишет причину рядом с кнопками (**No sound: …**
в роликах и Movies, **Cannot play: …** в строке состояния у звуковых банков)
и идёт дальше без звука.

## 1.3 Сборка из исходников

Нужен Rust (rustup; версия зафиксирована в `rust-toolchain.toml` и ставится
сама), компилятор C, `pkg-config` и заголовки ALSA (звук редактора включён
по умолчанию):

| Система | Пакеты |
|---|---|
| Debian, Ubuntu, Mint | `sudo apt install build-essential pkg-config libasound2-dev` |
| Fedora | `sudo dnf install gcc pkgconf-pkg-config alsa-lib-devel` |
| Arch | `sudo pacman -S base-devel alsa-lib` |

```sh
git clone https://github.com/Darkildo/doctor_toolkit
cd doctor_toolkit
cargo build --release                       # target/release/doctor-toolkit, -gui, -mods (со звуком)
tools/release.sh                            # архив как в релизе, в dist/ (с cargo-zigbuild — под glibc 2.31)
tools/install-desktop.sh                    # ярлыки в меню приложений для target/release
```

Без звука (нет заголовков ALSA или не нужна `libasound.so.2`): редактор
собирается с `--no-default-features`; других возможностей по умолчанию у
него нет, так что больше ничего включать не надо:

```sh
cargo build --release -p doctor-toolkit-gui --no-default-features
```

Такой редактор работает так же, только звуковые банки не проигрываются
(экспорт в WAV и замена работают), а ролики и Movies идут по таймеру кадров
с пометкой рядом с ползунком громкости. Командной строке и менеджеру модов
ALSA не нужна в любом случае.

Архив выпуска (`tools/release.sh`) всегда со звуком, ему тоже нужны
заголовки ALSA. С `cargo-zigbuild` (glibc 2.31) редактор линкуется с
`libasound.so` машины сборки, а проверка неопределённых символов самой
библиотеки отключена (`--allow-shlib-undefined`): ей нужна glibc машины
сборки, но у пользователя будет его собственная `libasound` со своей glibc.
Скрипт проверяет, что редактору нужны только версии символов `ALSA_0.9*`
(есть во всех alsa-lib с 2004 года), и пишет их в `BUILD-INFO.txt`.

## 1.4 Где программа хранит свои файлы

| Что | Где | Можно удалить? |
|---|---|---|
| Рабочее пространство | `<игра>/_doctor_toolkit/workspace/` | только если правки не нужны |
| Резервные копии оригиналов | `<игра>/_doctor_toolkit/backups/` | **нет**, пока игра изменена |
| Установленные моды | `<игра>/_doctor_toolkit/mods/` | через менеджер модов |
| Индекс игры (кэш) | `~/.cache/doctor-toolkit/` (`$DOCTOR_TOOLKIT_CACHE_DIR`) | да, построится заново |
| Журнал | `~/.local/state/doctor-toolkit/log.txt` (`$DOCTOR_TOOLKIT_LOG_DIR`) | да |
| Настройки и раскладка редактора | `~/.config/doctor-toolkit/settings.toml`, `layout.json` (`$DOCTOR_TOOLKIT_CONFIG_DIR`) | да, вернутся умолчания |
| Ярлыки и значки | `~/.local/share/applications/doctor-toolkit*.desktop`, `~/.local/share/icons/hicolor/*/apps/doctor-toolkit*` | `install-desktop.sh --uninstall` |

Сканер игры каталог `_doctor_toolkit` пропускает, игра его не читает.
