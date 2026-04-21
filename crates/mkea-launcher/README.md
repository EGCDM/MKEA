# mkEA Launcher

Эта ветка двигает проект в сторону нормального desktop-использования:

- `mkea-launcher.exe` — GUI-точка входа
- `mkea_player.exe` — отдельный runtime backend без консольного UX
- библиотека IPA хранится в `%LOCALAPPDATA%\mkEA`
- launcher умеет импортировать IPA, хранить manifest/build, запускать игру по клику и держать recent launches / встроенный просмотр логов

## Обычная release-сборка

```powershell
cargo build --release -p mkea-launcher --bin mkea-launcher
cargo build --release -p mkea-cli --bin mkea_player
```

После этого основной exe:

```text
target\release\mkea-launcher.exe
```

## Portable bundle для запуска по двойному клику

Используй один из скриптов в `scripts/`:

- `scripts/package_windows_release.cmd`
- `scripts/package_windows_release.ps1`

Они собирают release-бинарники и кладут их в `dist\mkEA\`:

- `mkea-launcher.exe`
- `mkea_player.exe`
- `mkEA Launcher.cmd`
- `README_PORTABLE.txt`

Дальше пользователь запускает **только** `mkea-launcher.exe` или `mkEA Launcher.cmd`.
