# Nozhik

Легковесная модульная IDE на Rust с нативной производительностью для Windows,
macOS и Linux. Цель проекта — инструменты для кода и баз данных уровня
PhpStorm/DataGrip с независимыми подсистемами и явными контрактами.

## Статус

**Roadmap Phase 0: архитектурный каркас реализован.** Четыре крейта, базовые
типы, асинхронная шина событий, настройки TOML/JSON, пустое системное окно,
тесты и CI. Редактор, рендерер, LSP и подключения к БД пока не реализованы.
Окно создаётся через winit; отрисовка содержимого на этом этапе отсутствует.

## Сборка и запуск

Установите текущий stable Rust через [rustup](https://rustup.rs/).
`rust-toolchain.toml` выбирает stable с rustfmt и Clippy; используется edition 2024.
`Cargo.lock` фиксирует разрешённые зависимости приложения и хранится в Git.

Платформенные инструменты:

| Система | Требования |
| --- | --- |
| Windows | Visual Studio Build Tools, компонент Desktop development with C++, Windows SDK; Rust MSVC toolchain |
| macOS | Xcode Command Line Tools: `xcode-select --install` |
| Linux | C/C++ toolchain, pkg-config, заголовки X11/Wayland/xkbcommon; активная графическая сессия |

Для Ubuntu/Debian:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libx11-dev libxkbcommon-dev libwayland-dev
```

Одинаковые команды в PowerShell, Terminal и Linux shell, из корня репозитория:

```sh
cargo build --workspace
cargo test --workspace --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p nozhik-app
cargo build --workspace --release
```

Бинарник: `target/release/nozhik-app` (`nozhik-app.exe` на Windows).
Закрытие окна завершает приложение и обработчик событий. Для тестов GUI
не требуется; для запуска нужен доступ к дисплею. CI проверяет три ОС;
наличие workflow само по себе не подтверждает успешный прогон.

## Архитектура

Стрелки обозначают зависимости; `app` — единственная точка сборки систем.

```text
nozhik-app ──► nozhik-config ──► nozhik-event-bus ──► nozhik-core
     └────────────────────────► nozhik-event-bus
                     nozhik-config ────────────────► nozhik-core
```

```text
crates/
  core/       # BufferId, Position, Span, ConfigKey
  event_bus/  # Event, EventBus; broadcast<Arc<Event>>
  config/     # Config, ConfigManager; Arc<RwLock<Config>>
  app/        # winit на главном потоке, Tokio workers
docs/ai/
  architecture.md
  coding-standards.md
  roadmap-progress.md
```

`core` не зависит от runtime, UI или файловой системы. Контракты событий
принадлежат `event_bus`; схема настроек принадлежит `config`.

## Настройки

`directories::ProjectDirs::from("dev", "Nozhik", "Nozhik")` определяет каталог.
Приложение выводит фактический путь в stderr после создания окна. Типичные пути:

- Windows: `%APPDATA%\Nozhik\Nozhik\config\settings.toml`.
- macOS: `~/Library/Application Support/dev.Nozhik.Nozhik/settings.toml`.
- Linux: `$XDG_CONFIG_HOME/nozhik/settings.toml` или `~/.config/nozhik/settings.toml`.

```toml
tab_width = 4
font_size = 14
theme = "dark"
```

Отсутствующий файл и отсутствующие поля используют defaults. Неизвестные поля,
повреждённый файл, неверные значения и ошибки доступа возвращают ошибку;
приложение сообщает её и завершает запуск. Исходный файл не перезаписывается.
При первом запуске настройки существуют только в памяти. Явный `save()`
создаёт каталог и сохраняет файл через временный файл в том же каталоге.
`ConfigManager::from_path` также поддерживает `.json`. Автоматического наблюдения
за файлом нет: вызывайте `reload()` на рабочем потоке.

## Документация для AI

Перед изменениями прочитайте [архитектуру](docs/ai/architecture.md),
[стандарты кода](docs/ai/coding-standards.md) и
[прогресс roadmap](docs/ai/roadmap-progress.md). При изменении публичного
контракта обновляйте их вместе с кодом.

Лицензия: [Apache-2.0](LICENSE).

