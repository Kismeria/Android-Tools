# Android Tools



Управление Android-телефоном с ПК: экран через scrcpy, приложения, файлы, телефон как веб-камера и микрофон.

Windows и Arch Linux. Интерфейс на русском и английском — язык выбирается при первом запуске.



*Control an Android phone from your PC: scrcpy screen, apps, files, phone as a webcam and microphone. Windows and Arch Linux, Russian and English UI — [English below](#english).*



## Установка в одну команду



**Windows 10/11** — PowerShell:

```powershell

irm https://raw.githubusercontent.com/Kismeria/Android-Tools/main/install.ps1 | iex

```

Скачает последнюю версию в `%LOCALAPPDATA%\Programs\Android Tools`, создаст ярлыки в меню «Пуск» и на рабочем столе и запустит программу. Или просто скачайте `Android-Tools.exe` из [Releases](../../releases/latest) — установка не нужна.



**Arch Linux** (и производные: EndeavourOS, Manjaro, CachyOS…):

```bash

curl -fsSL https://raw.githubusercontent.com/Kismeria/Android-Tools/main/install.sh | bash

```

Установит пакет `android-tools-gui` вместе с зависимостями (`android-tools`, `scrcpy`, `webkit2gtk-4.1`) и предложит поставить поддержку веб-камеры (`v4l2loopback`). Запуск — из меню приложений или командой `android-tools-gui`.



Без скрипта:

```bash

sudo pacman -U https://github.com/Kismeria/Android-Tools/releases/latest/download/android-tools-gui-x86_64.pkg.tar.zst

```



## Перед первым подключением

1. На телефоне: «Настройки → О телефоне» → 7 раз нажать «Номер сборки».

2. «Для разработчиков» → включить «Отладка по USB».

3. Подключить кабель и разрешить отладку на экране телефона.



## Возможности

| Раздел | Что умеет |

|---|---|

| Устройства | список, батарея и память, Wi‑Fi: подключение, сопряжение по коду, поиск в сети, переход USB → Wi‑Fi |

| Экран | окно scrcpy, профили, FPS, кодек, битрейт, звук (Android 10 — через [sndcpy](https://github.com/rom1v/sndcpy)), запись, HID‑мышь и HID‑клавиатура, кнопки телефона |

| Камера | телефон как веб-камера «Android Tools Camera», в том числе iPhone через Safari |

| Микрофон | телефон как микрофон «Android Tools Microphone»: усиление, шумовой порог, ограничитель, моно, прослушивание на ПК, индикатор уровня |

| Приложения | названия и иконки прямо с телефона, установка `.apk/.apks/.xapk/.apkm` (и перетаскиванием), удаление, запуск, остановка, очистка, отключение, сохранение APK |

| Файлы | обзор памяти, скачивание и загрузка, папки, переименование, удаление |

| Утилиты | скриншот в файл и буфер, кнопки телефона, перезагрузка, ввод текста, ссылки, shell |

| Настройки | 12 дизайнов (Cyberpunk, Windows 95, Windows XP, Aqua, Terminal, Brutal, Glass, Material You, Synthwave, Game Boy, Paper, Modern) и 19 палитр в духе Omarchy, акцентный цвет, масштаб, язык, обновление программы |



### Камера

| Система | Как работает | Где видна |

|---|---|---|

| Windows 11 | системная камера Media Foundation | везде, включая приложение «Камера» |

| Windows 10 | DirectShow-камера ([softcam](https://github.com/tshino/softcam)) | Zoom, Discord, Teams, Telegram, OBS, Chrome, Edge |

| Arch Linux | scrcpy `--v4l2-sink` → `v4l2loopback` | все программы |



Установка камеры — раздел «Камера» → «Установить» (один раз, нужны права администратора).

Прямой доступ к камере телефона — Android 12+; на старых версиях работает режим «Совм.» через приложение камеры.



### iPhone как веб-камера

Приложения на iPhone не нужны. «Девайсы» → «Подключить iPhone»: программа показывает QR-код, iPhone открывает страницу в Safari и передаёт камеру на ПК по Wi‑Fi (или по кабелю с включённым «Режимом модема»). Картинка идёт в ту же «Android Tools Camera»: на вкладке «Камера» выберите источник iPhone.

Сертификат страницы самоподписанный: в Safari нажмите «Подробнее» → «Посетить этот веб-сайт». Подойдёт и любой другой телефон с браузером. Linux: нужен `ffmpeg` (ставится вместе со `scrcpy`).



### Микрофон

| Система | Как работает |

|---|---|

| Windows 10/11 | драйвер [VB-CABLE](https://vb-audio.com/Cable/) (бесплатный, подписан Microsoft); «Установить» скачивает его с vb-audio.com и переименовывает устройство в «Android Tools Microphone» |

| Arch Linux | источник PipeWire/PulseAudio (`pactl`, пакет `libpulse`), без прав администратора |



Звук берётся со встроенного микрофона телефона (Android 11+). Режимы: обычный, голосовой (шумо- и эхоподавление телефона), без обработки, видеозапись, распознавание речи.

В Discord, Zoom, OBS и других программах выберите устройство ввода «Android Tools Microphone».



### Обновления

«Настройки → Обновления»: на Windows программа скачивает новый exe и перезапускается, на Arch Linux ставит новый пакет через `pacman -U` (спросит пароль).



### Если не работают мышь и клавиатура

Xiaomi/Redmi/POCO: «Для разработчиков → Отладка по USB (настройки безопасности)».

OPPO/Realme/OnePlus: «Для разработчиков → Отключить мониторинг разрешений».

Или включите «HID‑мышь» и «HID‑клавиатуру» в разделе «Экран».



---



## English



### One-command install

**Windows 10/11** (PowerShell):

```powershell

irm https://raw.githubusercontent.com/Kismeria/Android-Tools/main/install.ps1 | iex

```

**Arch Linux** and derivatives:

```bash

curl -fsSL https://raw.githubusercontent.com/Kismeria/Android-Tools/main/install.sh | bash

```

Or download `Android-Tools.exe` / `android-tools-gui-x86_64.pkg.tar.zst` from [Releases](../../releases/latest).



### Before the first connection

Enable Developer options (tap "Build number" 7 times), turn on USB debugging, connect the cable and allow debugging on the phone.



### Webcam

Windows 11 — Media Foundation system camera; Windows 10 — DirectShow camera; Linux — `v4l2loopback`.

Install it once from the Camera tab. Direct camera access requires Android 12+; older phones use "Compat" mode through the camera app.



### Microphone

Windows uses the signed [VB-CABLE](https://vb-audio.com/Cable/) driver, downloaded from vb-audio.com on install and renamed to "Android Tools Microphone".

Linux uses a PipeWire/PulseAudio pipe source (`pactl` from `libpulse`), no root needed. Requires Android 11+.



### iPhone as a webcam

No app on the iPhone: Devices → "Connect iPhone" shows a QR code, Safari opens a page and streams the camera to the PC over Wi‑Fi (or USB with Personal Hotspot on). Choose iPhone as the source on the Camera tab. The page uses a self-signed certificate: tap "Show Details" → "visit this website".



### Themes and updates

Settings has 12 designs that restyle the whole interface (Cyberpunk, Windows 95, Windows XP, Aqua, Terminal, Brutal, Glass, Material You, Synthwave, Game Boy, Paper, Modern) and 19 Omarchy-style palettes.

Settings → Updates installs new releases: the exe replaces itself on Windows, `pacman -U` on Arch Linux.



### Mouse and keyboard do not work

Xiaomi/Redmi/POCO: Developer options → "USB debugging (Security settings)".

OPPO/Realme/OnePlus: Developer options → "Disable permission monitoring".

Or turn on "HID mouse" and "HID keyboard" on the Screen tab.



---



## Сборка / Build

```

npm install

npm run fetch-tools   # Windows: downloads adb and scrcpy into src-tauri/embed

npm run build         # → src-tauri/target/release/

```

Windows: Rust (MSVC), Visual Studio Build Tools with Windows SDK, Node.js.

Arch Linux: `packaging/arch/PKGBUILD` (`makepkg -si`); GitHub Actions builds the package for every release.



## Лицензии / Licenses

MIT — [LICENSE](LICENSE).

- [scrcpy](https://github.com/Genymobile/scrcpy) — Apache License 2.0.

- [softcam](https://github.com/tshino/softcam) — MIT (Windows 10 camera).

- [sndcpy](https://github.com/rom1v/sndcpy) — Apache License 2.0, not bundled: installed on Android 10 phones for screen sound.
- [Tektur](https://github.com/google/fonts/tree/main/ofl/tektur) — SIL Open Font License 1.1 (Cyberpunk theme font, `ui/fonts/Tektur-OFL.txt`).
- [VB-CABLE](https://vb-audio.com/Cable/) — donationware by VB-Audio, not bundled: downloaded from the vendor on install (Windows microphone).

- [Fluent UI System Icons](https://github.com/microsoft/fluentui-system-icons) — MIT (AT Icons font).

- adb — [Android SDK Platform Tools](https://developer.android.com/tools/releases/platform-tools).

