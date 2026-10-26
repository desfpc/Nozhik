# Nozhik

Легковесная модульная IDE на Rust с нативной производительностью для Windows,
macOS и Linux. Цель проекта — инструменты для кода и баз данных уровня
PhpStorm/DataGrip с независимыми подсистемами и явными контрактами.

## Статус

**Roadmap Phase 5: Tree-sitter Syntax Highlighting & LSP Client Core реализован.** Десять крейтов,
нативная оболочка egui/eframe, ленивое дерево проекта, файловый I/O в фоне,
watcher, fuzzy-поиск по путям, вкладки, виртуализированный Rope-редактор
и встроенный Git-клиент. PHP: Tree-sitter подсветка, LSP-диагностика, completion,
hover и переход к определению. Терминал, запуск/отладка и подключения к БД
ещё не реализованы.

## Скачать без сборки

Готовые [macOS ARM64 и Windows x64 бинарники](binaries/README.md) хранятся в
`binaries/` вместе с SHA-256 исходников и файлов. После каждой правки исходников
обновляются обе сборки: локально через `scripts/update-binaries.py` и автоматически
после push через GitHub Actions. Инструкция запуска и ограничения — в каталоге.

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

Git backend собирает libgit2/OpenSSL из исходников: системный Git для локальных
операций не нужен. Для сборки также необходим Perl в PATH и make на Unix;
на Windows используйте Perl с совместимым MSVC окружением.

Для Ubuntu/Debian:

```sh
sudo apt-get update
sudo apt-get install -y build-essential perl pkg-config libx11-dev libxkbcommon-dev libwayland-dev
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
app ──► nozhik-ui ──► nozhik-language-support, nozhik-plugin-host, nozhik-vcs, nozhik-fs-model, nozhik-buffer, nozhik-core, nozhik-config, nozhik-event-bus
app ──► nozhik-config ──► nozhik-event-bus ──► nozhik-core
     └────────────────────────► nozhik-event-bus
                     nozhik-config ────────────────► nozhik-core
nozhik-buffer ──► nozhik-event-bus, nozhik-core
nozhik-fs-model ──► nozhik-event-bus
nozhik-vcs ──► git2 (vendored libgit2/OpenSSL)
nozhik-plugin-host ──► nozhik-event-bus, tokio, serde/serde_json, semver, directories
nozhik-language-support ──► tree-sitter/php, lsp-types, tokio, ropey, serde_json
```

```text
crates/
  core/       # BufferId, Position, Span, ConfigKey
  buffer/     # TextBuffer, CursorSet, History, SearchEngine; Ropey
  event_bus/  # Event, EventBus; broadcast<Arc<Event>>
  config/     # Config, ConfigManager; Arc<RwLock<Config>>
  fs_model/   # lazy tree, ignore, index, watcher, background I/O
  vcs/        # Git worker, status, index, diff, history, branches, remotes
  plugin_host/ # manifests, process JSON-RPC host, action/keymap registries
  language_support/ # incremental PHP syntax, stdio LSP client, background workers
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
текущей сессии. Боковые кнопки открывают Project и Git, остальные tool windows
пока представлены заглушками.

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

## Встроенный Git-клиент

Откройте папку внутри существующего Git-репозитория. Кнопка `⑂` на левом
тулстрипе или **Ctrl/Cmd+Shift+G** открывает Changes/History. Если открыта
подпапка репозитория, Git-панель показывает весь репозиторий; пути в ней
относительны его корню.

- **Changes**: фильтр по пути, цветные статусы, staging/unstaging файла и Select All.
  Чекбоксы управляют настоящим Git index. Промежуточный чекбокс означает наличие
  staged и unstaged изменений одновременно; снятие снимает staging, повторная
  установка включает текущую дисковую версию файла целиком.
- **Commit**: обязательный Summary и необязательный Description. Кнопка
  `Commit N files to branch` или **Ctrl/Cmd+Enter** в Git-панели записывает index.
  Нужна настроенная Git identity (`user.name` и `user.email`). Успех очищает поля,
  обновляет статус и History; оставшиеся unstaged изменения сохраняются в списке.
- **Diff Viewer**: unified hunks, старая/новая нумерация, зелёные добавления и
  красные удаления, моноширинный шрифт. По умолчанию сравниваются HEAD и рабочее
  дерево; **Staged only** показывает именно содержимое будущего коммита.
- **History**: карточки коммитов, автор/относительное время, описание, полный и
  короткий SHA с копированием, затронутые файлы и их исторические diff. Merge
  commits сравниваются с первым родителем. История загружается по 100 записей
  до 10 000; карточки и строки diff виртуализированы.
- **Ветки и remotes**: верхняя панель и заголовок показывают текущую ветку.
  Переключатель позволяет искать локальные ветки, создавать и переключать их.
  Toolbar предлагает Fetch/Push origin, меню `…` также содержит Pull origin и Refresh.
  Отображаются ahead/behind относительно upstream. Push не использует force;
  для новой ветки устанавливается upstream. `origin` должен быть настроен заранее.

Git status обновляется после сохранений/уведомлений watcher с debounce 200 мс,
а для открытого репозитория дополнительно проверяется раз в 3 секунды, включая
внешние изменения `.git`. Status, diff, index, history, commit и сеть исполняются
в выделенном worker; результаты будят UI. SSH использует агент, HTTPS —
настроенный credential helper. Секреты не вводятся и не хранятся в интерфейсе.

В контекстном меню файла **Discard Changes…** запрашивает подтверждение перед
восстановлением его index/дисковой версии из HEAD (untracked-файл удаляется).
Переименование восстанавливает старый путь и удаляет новый; если старый путь уже
занят, операция отклоняется. **Ignore File** добавляет точный экранированный путь
в корневой `.gitignore`; уже tracked-файл это не удаляет из index.

Ветки переключаются и Pull выполняется только при чистом index/рабочем дереве;
Pull поддерживает fast-forward, расхождение веток возвращает ошибку. Merge/rebase,
разрешение конфликтов, submodule-операции, hooks и GPG signing пока не поддерживаются.
Git действует на дисковые версии: несохранённый текст редактора в коммит не входит.
После checkout/pull/discard буферы редактора сохраняются в памяти без автоматического
reload — это также относится к последующему сохранению устаревшего буфера.
Binary/oversized diff показывает заглушку; текстовая выдача ограничена 2 MiB,
50 000 строк и 4096 символами на строку с явной отметкой о сокращении.

## Расширяемость и горячие клавиши

`nozhik-plugin-host` обнаруживает `plugin.json` в подпапках `~/.nozhik/plugins/`
и `<project>/.nozhik/plugins/`. Расширение поставляется отдельно от IDE:
строгий манифест регистрирует языковые ассоциации, команды, defaults настроек
и описания вкладок; optional executable запускается отдельным процессом
по событиям открытия файла или вызова команды. SemVer проверяет минимум IDE
и зависимости, конфликты и циклы видны в диагностике.

Все вызовы расширений выполняются в фоновом host с JSON-RPC/stdio, очередью,
таймаутом 3 секунды и лимитом ответа 64 KiB. Падение расширения переводит его
в Failed и удаляет вклады. Процесс работает с правами пользователя;
OS sandbox и произвольные языковые/LSP/БД API для внешних плагинов ещё не реализованы.
Встроенная PHP-поддержка использует отдельный language_support и LSP-протокол.

**Settings → Plugins…** или команда **Plugins** в Search Everywhere открывает
список с версией, автором, статусом и переключателем Enabled. Можно включать
и отключать расширения без перезапуска; зависимые плагины отключаются вместе
с зависимостью. **Rescan installed plugins** перечитывает каталоги. В текущей
версии состояния переключателей действуют до следующего scan/запуска.
Раздел Extension tabs показывает декларативные вкладки расширений.

**Search Everywhere → Actions** динамически читает потокобезопасный реестр
действий, фильтрует доступность по контексту и показывает shortcuts.
Enter запускает выбранную команду. `when` поддерживает `editorOpen`,
`editorFocus`, `selectedFile`, `projectOpen`, `&&` и `!`.

Файл `~/.nozhik/keymap.json` переключает профили `JetBrains`, `VSCode`, `Custom`
без перезапуска (проверка раз в секунду):

```json
{
  "profile": "VSCode",
  "bindings": [
    {
      "chord": { "key": "R", "primary": true, "alt": true },
      "action": "php.runScript",
      "when": "editorOpen"
    }
  ]
}
```

`primary` — Ctrl на Windows/Linux и Cmd на macOS. Последний binding одинакового
сочетания заменяет предыдущий; ошибочный reload сохраняет текущий профиль.
VSCode использует Ctrl/Cmd+Shift+P для поиска, JetBrains — Ctrl/Cmd+Shift+A;
Double Shift доступен в обоих. Внутренние сочетания текстового редактора
и Git commit пока принадлежат соответствующим views.

Полный манифест, IPC-протокол и примеры API:
[руководство автора расширений](docs/ai/plugin-authoring-guide.md).


## Поддержка языков: PHP

Встроенная конфигурация `org.nozhik.php` связывает `.php` и `.phtml` с PHP.
Tree-sitter и bundled `highlights.scm` работают без внешнего сервера: ключевые слова,
строки, комментарии, функции, типы и переменные окрашены палитрой Darcula/New UI.
Разбор выполняется в отдельном worker с повторным использованием изменённого дерева;
UI применяет только диапазоны актуальной ревизии и рисует видимые строки.
Вложенные HTML/CSS/JavaScript грамматики пока не подключены.

Для диагностики и интеллектуальных функций установите один LSP-сервер и добавьте
его executable в `PATH`, с которым запускается Nozhik:

- [Phpactor](https://phpactor.readthedocs.io/en/master/usage/getting-started.html):
  IDE запускает `phpactor language-server` ([stdio](https://phpactor.readthedocs.io/en/master/lsp/running.html)).
- [Intelephense](https://github.com/bmewburn/intelephense-docs/blob/master/installation.md):
  установите Node.js/npm, затем `npm install -g intelephense`; IDE запускает
  `intelephense --stdio`.

Phpactor имеет приоритет при наличии обоих. На Windows ищутся `.exe`, `.cmd`,
`.bat`; на Unix требуется executable permission. После изменения PATH перезапустите
IDE. При отсутствии сервера статус и stderr показывают инструкцию установки,
подсветка остаётся доступной. Сервер запускается при первом PHP-документе проекта;
смена проекта пересоздаёт сервер. Файл для LSP может ещё не существовать на диске.

Ошибки/предупреждения отображаются волнистыми линиями и маркерами gutter;
наведение на диапазон показывает сообщение. Статус-бар считает диагностики
открытых PHP-вкладок. Ввод букв, `$`, `\`, `->`, `::` запрашивает completion после
200 мс покоя. Стрелки выбирают подсказку, Enter/Tab принимают, Esc закрывает;
поддерживаются серверные textEdit и дополнительные правки (например imports)
одной undo-транзакцией. При наведении показывается hover с базовым Markdown
(заголовки, код, жирный текст, списки). Ctrl/Cmd+клик или F12 открывает первый
результат definition и прокручивает редактор к строке.

LSP использует UTF-16, stdio JSON-RPC Content-Length, initialize/initialized,
didOpen/didChange/didClose и shutdown/exit. didChange объединяется за 200 мс;
перед интерактивным запросом отправляется актуальная версия. Full и incremental
синхронизация выбираются по capabilities сервера (incremental — замена прежнего
документа целиком). Запросы имеют таймаут 5 секунд; зависший сервер не блокирует
рендеринг и Tree-sitter. При выходе cleanup ограничен по времени, затем процесс
принудительно завершается. LSP-сервер работает с правами пользователя.

Текущий объём — LSP client core: snippets/resolve, code actions, rename/refactoring,
semantic tokens, диагностика закрытых файлов, пользовательские LSP settings и
автоматический restart после падения пока отсутствуют. PHP contribution встроен
и не управляется переключателем executable-плагинов в Settings. Локальная библиотека
предоставляет `LanguageService::with_server` для явного пути/аргументов сервера.
