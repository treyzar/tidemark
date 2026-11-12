# Tidemark

Ранее — Win Island. Имя `win-island.exe` и путь к данным сохранены для совместимости. Нативные настройки с четырьмя разделами, собственная иконка и два прибрежных фона (папка `wallpapers`).

[Скачать последнюю версию](https://github.com/treyzar/tidemark/releases/latest) · [Изменения](CHANGELOG.md)

Dynamic Island для Windows 11: остров у верхнего края экрана. Показывает время и погоду, музыку (обложка, текст песни, управление), историю буфера обмена, скриншоты с редактором, OCR, QR, запись экрана, Wi‑Fi/Bluetooth/VPN, таймеры и заметки, поиск по Alt+Space. Карточки с вопросами для Claude Code и opencode.

Нативное приложение на Rust и Win32, без браузерного движка.

## Установка

Нужна Windows 10 (2004) или новее; рассчитано на Windows 11. Права администратора не нужны.

1. Скачайте с вкладки Releases:
   - `tidemark-<версия>-setup.exe` — установщик (ставит в `%LOCALAPPDATA%\Programs\Tidemark`, добавляет ярлыки и, по желанию, автозапуск);
   - или `tidemark-<версия>-win-x64.zip` — portable: распакуйте куда угодно и запустите `win-island.exe`.
2. Windows SmartScreen может предупредить, что программа неизвестна: exe не подписан. «Подробнее» → «Выполнить в любом случае».
3. Значок острова появится в области уведомлений (трее).

Удаление: «Параметры → Приложения» (для установщика) или просто удалите папку (для portable; перед этим отключите автозапуск в настройках). Данные лежат в `%APPDATA%\win-island` — их можно удалить вручную.

## Настройки

Правый клик по значку в трее → «Настройки…» (или `win-island.exe --settings`, это же ярлык «Настройки Tidemark» в меню «Пуск»). В окне: тема, акцентный цвет, фон, прозрачность, задержка раскрытия, плитки на «Главной», поведение острова, город для погоды, адрес поиска и **автозапуск вместе с Windows**. «Применить» меняет остров сразу, без перезапуска.

Всё хранится в `%APPDATA%\win-island\config.toml`; там есть ещё параметры без окна (размеры острова, список приложений в доке). После правки файла вручную: трей → «Перечитать config.toml».

Автозапуск — запись `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Путь обновляется при каждом запуске, так что перенос или обновление программы его не ломают.

## Горячие клавиши

Интернет-поиск использует Google. Прежний стандартный адрес Яндекса обновляется при загрузке конфигурации. Свой адрес можно задать в разделе «Система и данные».

| Клавиши | Действие |
|---|---|
| Alt+Space | поиск (приложения, файлы, калькулятор, интернет) |
| Ctrl+Alt+T | светлая / тёмная тема |

## Claude Code и opencode

Остров показывает запросы разрешений и вопросы агента карточкой с кнопками.

**Claude Code.** В `integrations\claude-code\hooks.template.json` замените `@HOOK@` на путь к `island-hook.exe` (в JSON — с `/`, например `C:/Users/you/AppData/Local/Programs/Tidemark/island-hook.exe`) и перенесите блок `hooks` в `~/.claude/settings.json`.

**opencode.** Скопируйте `integrations\opencode\win-island.js` в `~/.config/opencode/plugins/`. Установщик сам задаёт переменную `WIN_ISLAND_HOOK`; для portable-версии задайте её вручную.

## Сборка из исходников

Нужны Rust (MSVC toolchain), Windows и Visual Studio Build Tools с компонентами Desktop development with C++ и Windows SDK (включая `rc.exe`).

```powershell
cargo build --release          # target\release\win-island.exe, island-hook.exe
cargo test --release           # логика, регрессионные проверки и нативные курсоры
.\scripts\package.ps1          # dist\*.zip и, если установлен Inno Setup, dist\*-setup.exe
```

Установщик собирается [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`). Релиз на GitHub собирается автоматически при пуше тега `v*` (`.github/workflows/release.yml`).

## Linux

Не поддерживается: остров целиком построен на Win32, Direct2D и WinRT. Версия для Ubuntu потребовала бы отдельной программы.

Замеры ресурсов по этапам — в `PLAN.md`.


Дизайн настроек: [unslop](https://github.com/heydamir/unslop) by Damir Sokolovsky, CC BY 4.0. Ресурсы и промпты — в [описании дизайна](docs/design/tidemark.md).
