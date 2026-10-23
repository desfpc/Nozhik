# Nozhik

Легковесная модульная IDE на Rust с нативной производительностью для Windows,
macOS и Linux. Цель проекта — инструменты для кода и баз данных уровня
PhpStorm/DataGrip с независимыми подсистемами и явными контрактами.

## Статус

**Roadmap Phase 2: UI Shell & Windowing реализован.** Шесть крейтов,
нативная оболочка egui/eframe (Glow/OpenGL), тёмная тема New UI, переключаемые
док-панели, вкладки, виртуализированный Rope-редактор и Search Everywhere.
Дерево и документы демонстрационные, хранятся в памяти. VFS, LSP, Git,
терминал, запуск/отладка и подключения к БД ещё не реализованы.

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
cargo run -p app
cargo build --workspace --release
```

Бинарник: `target/release/app` (`app.exe` на Windows).
Закрытие окна завершает приложение и обработчик событий. Для тестов GUI
не требуется; для запуска нужен доступ к дисплею. CI проверяет три ОС;
наличие workflow само по себе не подтверждает успешный прогон.

## Архитектура

Стрелки обозначают зависимости; `app` — единственная точка сборки систем.

```text
app ──► nozhik-ui ──► nozhik-buffer, nozhik-core, nozhik-config, nozhik-event-bus
app ──► nozhik-config ──► nozhik-event-bus ──► nozhik-core
     └────────────────────────► nozhik-event-bus
                     nozhik-config ────────────────► nozhik-core
nozhik-buffer ──► nozhik-event-bus, nozhik-core
```

```text
crates/
  core/       # BufferId, Position, Span, ConfigKey
  buffer/     # TextBuffer, CursorSet, History, SearchEngine; Ropey
  event_bus/  # Event, EventBus; broadcast<Arc<Event>>
  config/     # Config, ConfigManager; Arc<RwLock<Config>>
  ui/         # egui shell, theme, TextEditorView, Search Everywhere
  app/        # eframe на главном потоке, Tokio workers
docs/ai/
  architecture.md
  coding-standards.md
  roadmap-progress.md
```

`core` не зависит от runtime, UI или файловой системы. Контракты событий
принадлежат `event_bus`; схема настроек принадлежит `config`.

## Текстовый движок

`nozhik-buffer` зависит от `core` и `event_bus` и работает без UI, runtime
и файлового I/O. Ropey хранит UTF-8 по чанкам; снимки используют copy-on-write.
`line` и `lines` возвращают заимствованные `RopeSlice` для виртуального viewport.
Правки и поиск используют байтовые диапазоны с проверкой границ Unicode scalar;
комбинированные символы и эмодзи сохраняются, навигация по графемам будет в UI.
LF и CRLF сохраняются без нормализации; одиночный CR считается текстом.

Транзакция атомарно заменяет непересекающиеся диапазоны исходного текста.
Курсоры сохраняют направление выделения и сливаются при коллизиях, включая
соприкосновение границ. Undo/redo восстанавливает текст и курсоры, поддерживает
явные группы ввода и точку сохранения (`is_dirty`, `mark_saved`).
`SearchEngine` ищет литералы и Regex непосредственно по чанкам через
`regex-cursor`, в том числе через границы строк, без промежуточного `String`.
События `BufferEvent::Modified` и `CursorChanged` содержат ревизию;
после отставания подписчик получает актуальный `snapshot()` у владельца.

```rust
use nozhik_buffer::{BufferId, SearchEngine, TextBuffer};

let mut buffer = TextBuffer::load_from_str(BufferId(1), "Привет\n");
buffer.insert(12, " 🦀")?; // байтовое смещение перед LF
let matches = SearchEngine::literal("🦀")?.find_all(&buffer);
assert_eq!(matches, vec![13..17]);
buffer.undo()?;
assert!(!buffer.is_dirty());
```

Загрузка/сохранение файлов и реестр открытых документов остаются задачами
интеграции: вызывающий код выделяет уникальные `BufferId` и отмечает состояние
сохранённым только после успешной записи. Ядро подключено к TextEditorView;
ввод проходит через EditTransaction.

## Настройки

`directories::ProjectDirs::from("dev", "Nozhik", "Nozhik")` определяет каталог.
Приложение выводит фактический путь в stderr перед запуском окна. Типичные пути:

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

## Графический интерфейс

Запуск: `cargo run -p app`. Кликните по тексту для фокуса редактора.
Ввод, Backspace/Delete, Enter/Tab, стрелки, Home/End, Shift-выделение,
Ctrl/Cmd+A, Ctrl/Cmd+Z и Ctrl/Cmd+Shift+Z работают с TextBuffer.
Закрытие вкладки сохраняет демонстрационный буфер в памяти до выхода;
дерево Project и поиск позволяют открыть его снова. Записи файлов пока нет.

Search Everywhere: Double Shift (интервал до 400 мс), Ctrl/Cmd+Shift+A
или лупа на тулбаре. Стрелки выбирают результат, Enter открывает, Esc закрывает.
Ctrl/Cmd+Shift+O и Ctrl/Cmd+E пока открывают тот же поиск. Фильтры Classes/Symbols
пусты до появления индекса; Files/Text работают с демонстрационными документами,
Actions — с зарегистрированными действиями. Include non-project items зарезервирован
для будущего индекса. Шестерёнка открывает настройки размера шрифта и отступа
текущей сессии. Боковые и нижние кнопки переключают панели-заглушки.
