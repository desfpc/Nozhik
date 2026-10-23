# Nozhik

Легковесная модульная IDE на Rust с нативной производительностью для Windows,
macOS и Linux. Цель проекта — инструменты для кода и баз данных уровня
PhpStorm/DataGrip с независимыми подсистемами и явными контрактами.

## Статус

**Roadmap Phase 3: файловая система и Project Model реализованы.** Семь крейтов,
нативная оболочка egui/eframe, ленивое дерево проекта, файловый I/O в фоне,
watcher, fuzzy-поиск по путям, вкладки и виртуализированный Rope-редактор.
LSP, Git, терминал, запуск/отладка и подключения к БД ещё не реализованы.

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
app ──► nozhik-ui ──► nozhik-fs-model, nozhik-buffer, nozhik-core, nozhik-config, nozhik-event-bus
app ──► nozhik-config ──► nozhik-event-bus ──► nozhik-core
     └────────────────────────► nozhik-event-bus
                     nozhik-config ────────────────► nozhik-core
nozhik-buffer ──► nozhik-event-bus, nozhik-core
nozhik-fs-model ──► nozhik-event-bus
```

```text
crates/
  core/       # BufferId, Position, Span, ConfigKey
  buffer/     # TextBuffer, CursorSet, History, SearchEngine; Ropey
  event_bus/  # Event, EventBus; broadcast<Arc<Event>>
  config/     # Config, ConfigManager; Arc<RwLock<Config>>
  fs_model/   # lazy tree, ignore, index, watcher, background I/O
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
Закрытие вкладки сохраняет буфер, включая несохранённые изменения, в памяти до
выхода; дерево Project и поиск позволяют открыть его снова. При выходе буферы
не сохраняются автоматически.

Search Everywhere: Double Shift (интервал до 400 мс), Ctrl/Cmd+Shift+A
или лупа на тулбаре. Стрелки выбирают результат, Enter открывает, Esc закрывает.
Ctrl/Cmd+Shift+O и Ctrl/Cmd+E пока открывают тот же поиск. Фильтры Classes/Symbols
пусты до появления синтаксического индекса; Files ищет по всем путям проекта,
Text — по содержимому буферов в памяти, Actions — по зарегистрированным действиям.
Include non-project items зарезервирован для будущего индекса. Шестерёнка открывает настройки размера шрифта и отступа
текущей сессии. Боковые и нижние кнопки переключают панели-заглушки.

## Работа с проектами и файлами

`cargo run -p app -- /path/to/project` открывает указанную рабочую папку;
без аргумента открывается текущая директория. В Windows передайте обычный путь,
например `cargo run -p app -- 'C:\Projects\demo'`. Для смены проекта в панели
Project введите путь папки и нажмите **Open**. Смена проекта сохраняет буферы
предыдущего проекта в памяти, включая несохранённые изменения.

Клик по `>` / `v` раскрывает или сворачивает каталог; его непосредственные дети
загружаются в фоне при первом раскрытии. Папки идут перед файлами, обе группы
отсортированы по алфавиту. Двойной клик по файлу или Enter на выбранном файле
открывает вкладку; повторное открытие активирует существующий буфер.

**Ctrl/Cmd+S** сохраняет текущий файл. Запись выполняется в фоне через временный
файл в том же каталоге с сохранением permissions, затем файл заменяется.
Dirty снимается только после успеха и только для записанной ревизии; правки,
сделанные во время сохранения, остаются dirty. Ошибки видны в status bar и stderr.
Нужны права записи на файл и на его каталог. Удалённый до сохранения файл
возвращает ошибку. Межпроцессных блокировок и слияния внешних правок пока нет.

В Search Everywhere выберите **Files**, введите часть относительного пути,
выберите результат стрелками и нажмите Enter. Fuzzy-поиск выполняется в фоне
по всему индексу, включая ещё не раскрытые каталоги; выдача ограничена 100 файлами.
Пути в поиске отображаются через `/`, запросы принимают и `\`, и `/`.

Дерево, индекс и watcher учитывают `.gitignore` (включая вложенные правила,
даже без Git-репозитория), `.ignore` и глобальные Git ignore rules.
`.git`, `target`, `node_modules`, `vendor` всегда исключаются.
Символические ссылки при обходе не раскрываются. Watcher объединяет изменения
одного пути за 150 мс; create/remove/rename обновляют дерево и индекс.
Изменения открытых файлов сообщаются в статусе; буферы сохраняют свой текст,
автоматического reload/merge нет. Собственные сохранения также видны watcher.

NUL в первом килобайте показывает **Binary file not supported**; такой буфер
нельзя редактировать или сохранять. Невалидный UTF-8 возвращает ошибку открытия.
