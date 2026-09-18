# Android Tools

Управление Android-телефоном с ПК: экран через scrcpy, приложения, файлы, телефон как веб-камера.
Windows и Arch Linux. Интерфейс на русском и английском — язык выбирается при первом запуске.

*Control an Android phone from your PC: scrcpy screen, apps, files, phone as a webcam. Windows and Arch Linux, Russian and English UI — [English below](#english).*

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
| Экран | окно scrcpy, профили, FPS, кодек, битрейт, звук, запись, HID‑мышь и HID‑клавиатура |
| Камера | телефон как веб-камера «Android Tools Camera» |
| Приложения | установка `.apk/.apks/.xapk/.apkm` (и перетаскиванием), удаление, запуск, остановка, очистка, отключение, сохранение APK |
| Файлы | обзор памяти, скачивание и загрузка, папки, переименование, удаление |
| Утилиты | скриншот в файл и буфер, кнопки телефона, перезагрузка, ввод текста, ссылки, shell |

### Камера
| Система | Как работает | Где видна |
|---|---|---|
| Windows 11 | системная камера Media Foundation | везде, включая приложение «Камера» |
| Windows 10 | DirectShow-камера ([softcam](https://github.com/tshino/softcam)) | Zoom, Discord, Teams, Telegram, OBS, Chrome, Edge |
| Arch Linux | scrcpy `--v4l2-sink` → `v4l2loopback` | все программы |

Установка камеры — раздел «Камера» → «Установить» (один раз, нужны права администратора).
Прямой доступ к камере телефона — Android 12+; на старых версиях работает режим «Совм.» через приложение камеры.

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
- [Fluent UI System Icons](https://github.com/microsoft/fluentui-system-icons) — MIT (AT Icons font).
- adb — [Android SDK Platform Tools](https://developer.android.com/tools/releases/platform-tools).
