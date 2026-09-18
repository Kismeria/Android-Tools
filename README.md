# Android Tools

Управление Android-телефоном с ПК: Tauri 2 (Rust) + adb (SDK Platform Tools) + scrcpy. Один файл `Android Tools.exe`.

## Сборка
```
npm install
npm run fetch-tools  # скачивает adb и scrcpy в src-tauri/embed
npm run build        # → src-tauri/target/release/Android Tools.exe
```
Готовый exe — в [Releases](../../releases).
Нужны: Rust (MSVC), Visual Studio Build Tools с Windows SDK (C++ DLL камеры собирается в `build.rs`), Node.js.

## Что внутри exe
- интерфейс (`ui/`);
- `embed/platform-tools.zip` и `embed/scrcpy.zip` — при первом запуске распаковываются в `%LOCALAPPDATA%\AndroidTools\bin`;
- `AndroidToolsCam.dll` — источник видео Media Foundation для системной камеры.

## Камера Windows
- **Windows 11** — системная камера Media Foundation (видна везде, включая приложение «Камера»).
- **Windows 10** — DirectShow-камера на основе [softcam](https://github.com/tshino/softcam) (`src-tauri/softcam`, MIT): видна в Zoom, Discord, Teams, Telegram, OBS, Chrome, Edge; 64- и 32-битные версии.

«Камера» → «Установить» (UAC, один раз):
1. включает службы `FrameServer` / `FrameServerMonitor`, если они отключены;
2. копирует DLL в `%ProgramData%\AndroidTools\Camera`, регистрирует COM-класс;
3. создаёт системную камеру `MFCreateVirtualCamera` → **Android Tools Camera** (видна в «Камере» Windows, Zoom, Teams, Discord, браузерах).

Поток: scrcpy-server (H.264) → Rust → декодер Media Foundation → поворот/зеркало/масштаб → общая память `Global\AndroidToolsCamFrame` → DLL в Frame Server. Если трансляции нет, камера показывает заглушку.

Прямой доступ к камере телефона (`video_source=camera`) — Android 12+. На более старых телефонах используется режим «Совм.»: открывается приложение камеры и транслируется экран.

## Структура
| Путь | Назначение |
|---|---|
| `src-tauri/src/adb.rs` | обёртка adb: устройства, приложения, файлы, кнопки |
| `src-tauri/src/tools.rs` | встроенные adb/scrcpy, обновление |
| `src-tauri/src/mirror.rs` | окно scrcpy |
| `src-tauri/src/camera/` | поток камеры, декодер, трансформация, общая память, установка |
| `src-tauri/vcam/` | C++ DLL источника камеры |
| `ui/` | HTML/CSS/JS интерфейс |

## Лицензия
MIT — см. [LICENSE](LICENSE).

## Лицензии сторонних компонентов
- [scrcpy](https://github.com/Genymobile/scrcpy) — Apache License 2.0.
- [softcam](https://github.com/tshino/softcam) — MIT (DirectShow-камера для Windows 10).
- adb — часть [Android SDK Platform Tools](https://developer.android.com/tools/releases/platform-tools).
