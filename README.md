# Nozhik

Легковесная модульная IDE на Rust с нативной производительностью для Windows,
macOS и Linux. Цель проекта — инструменты для кода и баз данных уровня
PhpStorm/DataGrip с независимыми подсистемами и явными контрактами.

## Статус

**v0.1.0-RC1 — Feature Complete. Этап 9 реализован.** Тринадцать крейтов,
нативная оболочка egui/eframe, ленивое дерево проекта, файловый I/O в фоне,
watcher, fuzzy-поиск по путям, вкладки, виртуализированный Rope-редактор
и встроенный Git-клиент. PHP: Tree-sitter подсветка, LSP-диагностика, completion,
hover и переход к определению. Встроенный PTY-терминал поддерживает локальный
шелл, ANSI-цвета, вкладки и TUI-программы. SSH: удалённый шелл, SFTP-редактор
и локальные туннели. Встроенный клиент PostgreSQL, MySQL/MariaDB и SQLite:
дерево схем, SQL-консоль и виртуализированный Data Grid. Запуск/отладка через
task runner ещё не реализованы.

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
app ──► nozhik-ui ──► nozhik-database, nozhik-remote-ssh, nozhik-terminal, nozhik-language-support, nozhik-plugin-host, nozhik-vcs, nozhik-fs-model, nozhik-buffer, nozhik-core, nozhik-config, nozhik-event-bus
app ──► nozhik-config ──► nozhik-event-bus ──► nozhik-core
     └────────────────────────► nozhik-event-bus
                     nozhik-config ────────────────► nozhik-core
nozhik-buffer ──► nozhik-event-bus, nozhik-core
nozhik-fs-model ──► nozhik-event-bus
nozhik-vcs ──► git2 (vendored libgit2/OpenSSL)
nozhik-plugin-host ──► nozhik-event-bus, tokio, serde/serde_json, semver, directories
nozhik-language-support ──► tree-sitter/php, lsp-types, tokio, ropey, serde_json
nozhik-remote-ssh ──► russh (ring/Rust crypto), russh-sftp, tokio, serde, tempfile
nozhik-database ──► sqlx (PostgreSQL/MySQL/SQLite, Tokio/Rustls), nozhik-remote-ssh, keyring
nozhik-terminal ──► portable-pty, vt100 (vte), parking_lot, nix (Unix signals)
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
  database/    # SQL pools, SSH forwarding, typed results, catalog metadata, worker
  remote_ssh/  # SSH, host key pinning, SFTP cache, loopback tunnels
  terminal/    # PTY/ConPTY, VT100 grid/scrollback, asynchronous sessions
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

Файловый worker и реестр документов UI интегрированы с TextEditorView.
UI выделяет уникальные `BufferId`, ввод проходит через EditTransaction,
а успешный ответ сохранения очищает dirty только для той же ревизии.

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
Закрытие изменённой вкладки или окна показывает **Save All / Discard / Cancel**.
Save All ждёт подтверждения записи актуальных ревизий, в том числе SFTP и SQL.
Ошибка записи оставляет текст и окно открытыми. Закрытая вкладка освобождает
буфер/историю/результаты; Recent Files хранит до 30 путей для повторного открытия.

Search Everywhere: Double Shift (интервал до 400 мс), Ctrl/Cmd+Shift+A
или лупа на тулбаре. Стрелки выбирают результат, Enter открывает, Esc закрывает.
Ctrl/Cmd+Shift+O и Ctrl/Cmd+E пока открывают тот же поиск. Фильтры Classes/Symbols
пусты до появления синтаксического индекса; Files ищет по всем путям проекта,
Text — по содержимому буферов в памяти, Actions — по зарегистрированным действиям.
Include non-project items зарезервирован для будущего индекса. Шестерёнка открывает настройки размера шрифта и отступа
текущей сессии. Боковые кнопки открывают Project и Git, нижняя кнопка Terminal — PTY-терминал;
остальные tool windows пока представлены заглушками.

## Работа с проектами и файлами

`cargo run -p app -- /path/to/project` открывает указанную рабочую папку;
без аргумента открывается текущая директория. В Windows передайте обычный путь,
например `cargo run -p app -- 'C:\Projects\demo'`. Для смены проекта в панели
Project введите путь папки и нажмите **Open** либо нажмите **Choose folder…**
для стандартного окна выбора папки ОС. Эти элементы отображаются только без
открытого проекта; смена проекта всегда доступна в меню File. Под кнопками расположен список **Recent
Projects**, отсортированный по последнему успешному открытию. Клик открывает
выбранный проект; история сохраняется между запусками в `recent_projects.json`
рядом с `settings.toml`. Недоступный путь сообщает ошибку и не меняет историю.
Смена проекта сохраняет буферы предыдущего проекта в памяти, включая
несохранённые изменения.

**File → Open File / Folder…** сразу открывает системный диалог: на macOS можно
выбрать файл или папку в одном окне; Windows/Linux используют режим выбора папки,
а **File → Open File…** открывает системный выбор файла. **File → Recent Projects >**
показывает ту же историю в подменю, открывающемся при наведении.
**File → New…** предлагает выбрать родительскую папку ОС, затем ввести имя проекта.
После подтверждения создаётся новая папка и открывается как проект; существующие
папки не перезаписываются. Ошибка сохраняет форму для исправления имени.
**New File** по-прежнему создаёт отдельный файл по введённому пути.

Клик по стрелке `›` / `⌄` или строке папки раскрывает или сворачивает каталог; его непосредственные дети
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

Дерево показывает исключённые пути красным текстом на приглушённом красном фоне:
правила `.gitignore` (включая вложенные и без Git-репозитория), `.ignore`, глобальные
Git ignore rules, а также `target`, `node_modules`, `vendor`. Их содержимое доступно
через ленивое раскрытие, но исключено из поискового индекса. `.git` скрыта.
Тесты и настройки линтеров выделяются зелёным, включая потомков тестовых папок;
исключение имеет приоритет над зелёной подсветкой. Папки имеют иконки и стрелки
состояния, основные типы файлов — компактные значки, остальные — серый документ.
Символические ссылки при обходе не раскрываются. Watcher объединяет изменения
одного пути за 150 мс; create/remove/rename обновляют дерево и индекс, включая
отображаемые исключённые пути, кроме внутренних файлов `.git`.
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


## Встроенный терминал

Откройте проект и нажмите **Alt+F12** (на Mac — Option+F12, при необходимости Fn)
или **Terminal** на нижнем тулстрипе. Терминал открывается в текущем проектном
каталоге. В Search Everywhere → Actions доступно действие `ui.terminal`; его
сочетание можно изменить в пользовательском Keymap. JetBrains/VSCode profiles
включают Alt+F12 по умолчанию; Custom использует только явные bindings.

Верхняя строка панели содержит вкладки, **+** для новой сессии, **×/Close** для
завершения сессии и **—** для сворачивания панели. Скрытые сессии продолжают работать.
Новая вкладка получает имя Terminal N; OSC title от шелла/TUI может заменить его.
При завершении шелла вкладка закрывается, код выхода отображается в статусе.
При смене проекта существующие терминалы сохраняют свои cwd, новые открываются
в новом проекте. Изменение высоты панели/размера окна меняет геометрию PTY.

macOS/Linux используют `$SHELL` с login-аргументом `-l`, fallback — `/bin/zsh`
на macOS и `/bin/bash` на Linux. Windows использует `%COMSPEC%`, а при его отсутствии
ищет PowerShell, затем cmd.exe. Windows backend — ConPTY; нужен
[Windows 10 1809+ либо Windows 11](https://learn.microsoft.com/en-us/windows/console/createpseudoconsole).
Используется окружение процесса IDE, `TERM=xterm-256color`, `COLORTERM=truecolor`.
Можно запускать установленные `git`, `composer`, `cargo`, `vi`, `htop` и другие
консольные программы; установка этих программ остаётся задачей пользователя.

Клик по сетке передаёт терминалу фокус. Работают Unicode-ввод, Enter, Tab,
Backspace, Escape, стрелки, Home/End, Insert/Delete, PageUp/PageDown и F1–F12.
Ctrl+C отправляет interrupt, Ctrl+D — EOF, Ctrl+Z — suspend (Unix), Ctrl+L —
control-byte для очистки/перерисовки шеллом. Эти байты обрабатывает активная
программа/line discipline. Поддерживаются application cursor keys и bracketed paste.
Вставка из системного clipboard поступает в PTY; одна порция ограничена 64 KiB,
при переполнении очереди появляется ошибка вместо молчаливой потери ввода.

ANSI 16/256 и RGB-цвета, bold/dim/italic/underline/inverse, Unicode wide/combining
cells, статичный блочный курсор и альтернативный экран отрисовываются сеткой.
Колесо мыши просматривает историю до 3000 строк; новый ввод возвращает live screen.
В альтернативном экране история не прокручивается. Запуск, чтение, запись и resize
идут в фоновых потоках; UI получает готовый Arc-снимок без ожидания блокировок.
Закрытие вкладки/IDE останавливает шелл и освобождает PTY; на Unix также завершается
foreground process group. Явно отсоединённые пользовательские процессы не управляются
как отдельное дерево задач. Общий shutdown терминалов ограничен 3 секундами ожидания.

Текущие границы: до 16 сессий, 200×500 видимых ячеек; нет mouse-reporting в TUI,
выделения/копирования текста мышью, поиска по истории, persistent sessions и полного
набора xterm-расширений. Сетка не переформатирует историю как reflow-терминалы;
при resize применяются правила vt100. OSC clipboard и открытие ссылок не выполняются.


## Удалённая разработка: SSH, SFTP и туннели

Кнопка **⇄ Remote Host / SSH** на левом тулстрипе открывает список серверов.
**+ Add SSH host** сохраняет имя, Host, Port (22), User, Auth Type и правила
Local Port Forward. Профили находятся в `ssh_hosts.json` рядом с `settings.toml`
в каталоге конфигурации IDE; это пользовательский `config/ssh_hosts.json`,
а не файл в рабочем проекте. Повреждённый JSON не перезаписывается автоматически.
Форма позволяет добавлять, редактировать и удалять несколько правил проброса.
**Edit** изменяет сохранённый хост; **Delete…** удаляет его после подтверждения.
Смена имени/адреса/пользователя/метода входа удаляет прежний сохранённый пароль,
закрывает старое соединение и требует нового входа. Открытые файлы сохраняют
правки в памяти; после смены хоста их нужно заново открыть через SFTP для
дальнейшего удалённого сохранения. Удаление профиля удаляет его сохранённый
пароль, но оставляет отдельную запись доверия ключу сервера.

Отпечаток ключа вводить не нужно. При первом подключении Nozhik получает ключ
сервера и показывает **Trust SSH server?** с адресом и SHA256 fingerprint.
**Trust and Continue** запоминает ключ и продолжает подключение; **Cancel**
отменяет попытку, не сохраняя доверие и не отправляя пароль. При необходимости
отпечаток можно сверить с администратором сервера.

Подтверждённые ключи хранятся в `ssh_known_hosts.json` рядом с настройками,
отдельно для каждой пары host/port. Следующие соединения проверяются
автоматически. Изменившийся ключ вызывает блокировку и отдельный диалог;
старый ключ автоматически не заменяется. Повреждённый trust-файл не
перезаписывается. Старые профили с `host_key_sha256` продолжают проверять
свой pin; для новых профилей поле необязательно и отсутствует в форме.
Host certificates и политика доверия CA пока не поддерживаются.

Аутентификация: **Password**, **Private Key** (OpenSSH, RSA/Ed25519, путь
`~/.ssh/id_ed25519` или `~/.ssh/id_rsa`) и **SSH Agent**. На Unix используется
`SSH_AUTH_SOCK`, на Windows — `\\.\pipe\openssh-ssh-agent`. Пароль или
парольная фраза запрашиваются после проверки/подтверждения ключа сервера.
Поле Password получает фокус автоматически; Connect или Enter продолжает вход.
Пустая фраза допустима для незашифрованного ключа. Существующее соединение
переиспользуется.

Для сохранённого профиля доступна опция **Save password in system credential
store**. После успешной авторизации пароль/фраза сохраняются в **macOS Keychain**,
**Windows Credential Manager** или **Linux Secret Service** (разблокированный
сеанс GNOME Keyring/KWallet с Secret Service API). Следующий вход использует
сохранённый секрет после проверки ключа сервера. При неверном пароле или
недоступном хранилище IDE запрашивает новый ввод. **Forget password** удаляет
секрет из хранилища; отключение опции при успешном входе также удаляет старый.
Системное хранилище может запросить разрешение/разблокировку. Ошибка доступа
показывается пользователю; открытого файлового fallback нет.

В `ssh_hosts.json` хранится только boolean `save_password`, сами секреты
не попадают в JSON/логи; собственные временные буферы секретов очищаются при
освобождении. При тестировании ещё не сохранённого профиля пароль используется
только для текущего входа: сначала сохраните профиль, чтобы включить запоминание.

- **Test Connection** выполняет SSH-рукопожатие, проверку ключа и авторизацию.
- **Open Terminal** открывает вкладку `[SSH: имя]` в нижнем Terminal. Действуют
  ANSI-цвета, scrollback, alternate screen и клавиатура существующего терминала;
  геометрия передаётся серверу через `window-change`, TERM — `xterm-256color`.
  Закрытие вкладки закрывает её канал; **Disconnect** закрывает весь транспорт.
- **Browse SFTP** открывает домашний каталог (`.`). Раскрытие папки лениво
  загружает детей; двойной клик по файлу открывает обычный TextBuffer. Файл
  кэшируется в приватном временном каталоге; **Ctrl/Cmd+S** загружает текущий
  снимок обратно по SFTP. Dirty снимается только для сохранённой ревизии.
  При ошибке/обрыве правки остаются в памяти, локальная запись вместо SFTP
  не выполняется. Для внешних изменений используйте **Refresh**.
- **Start Tunnels** явно запускает сохранённые правила. Слушатель доступен
  только на `127.0.0.1`; `local_port: 0` выбирает свободный порт. Фактические
  адреса показываются в status bar. **Stop Tunnels**, Disconnect и выход из IDE
  закрывают слушатели и активные потоки. Пример: `127.0.0.1:15432` →
  `127.0.0.1:5432` с точки зрения SSH-сервера; клиент БД использует локальный порт.

SSH/SFTP работают в фоне. Connect/auth/channel/SFTP имеют deadline 15 секунд;
keepalive через 20 секунд закрывает транспорт после трёх пропущенных ответов.
Кэш содержит до 128 каталогов на соединение с TTL 30 секунд; upload и Refresh
сбрасывают его. Туннель ограничен 64 одновременными потоками.

Ограничения: удалённый редактор принимает UTF-8 до 16 MiB, бинарные файлы
отклоняются; SFTP upload перезаписывает файл с обрезанием, без атомарного rename,
блокировки и проверки внешних изменений. При сбое возможна частичная запись
на сервере. Remote workspace, удалённые LSP/Git, SSH config/ProxyJump,
keyboard-interactive/2FA и автоматический reconnect пока не реализованы.


SSH shell setup ждёт именно подтверждение PTY/shell: служебные
`WindowAdjusted` и `XonXoff` не считаются отказом, ранний вывод сохраняется
для терминала. Настоящие отказы различают `rejected PTY request` и
`rejected shell request`; причиной может быть запрет на стороне сервера.
Опциональная локальная проверка настоящим OpenSSH (Unix, требуется sshd):

```sh
cargo test -p nozhik-remote-ssh system_openssh_shell_and_sftp -- --ignored
```

Тест запускает отдельный sshd только на localhost с временными ключами и
конфигурацией, не меняя пользовательский `~/.ssh` или системный sshd.


## Встроенный клиент баз данных (Phase 8)

**Database** на правом тулстрипе открывает Database Explorer в стиле DataGrip.
Кнопка **+** создаёт подключение PostgreSQL, MySQL/MariaDB или SQLite (локальный
путь либо `:memory:`). Укажите имя, host/port/user/database; **Test Connection**
проверяет вход и чтение каталога, **Save** сохраняет профиль. Для существующего
профиля нажмите **Connect**; **Connect with password…** позволяет повторить вход
с новым паролем после отказа сохранённого. **↻** обновляет метаданные.

Профили находятся в `database_connections.json` рядом с `settings.toml` в каталоге
config пользователя. JSON содержит `name`, `driver` (`Postgres`/`Mysql`/`Sqlite`),
`host`, `port`, `user`, `database`, `ssh_profile_id` и `save_password`.
Сохранение атомарно; повреждённый JSON не перезаписывается. Пароли остаются
в памяти сессии. **Save password in system credential store** — явный opt-in
в Keychain / Windows Credential Manager / Linux Secret Service, доступный после
успешного теста именно этого профиля и пароля. Открытого пароля в JSON нет.

**Use SSH Tunnel** выбирает существующий SSH-профиль. Штатный SSH-flow проверяет
ключ хоста, при необходимости запрашивает доверие и пароль/ключ. После авторизации
создаётся отдельный loopback-туннель `127.0.0.1:0` к host/port БД; пул владеет
туннелем до отключения. SSH-профили и пароли БД независимы. SQLite работает локально.

Дерево показывает Online/Offline → схемы/базы → Tables/Views → колонки, типы,
nullable и PK. Контекстное меню таблицы: **Open Table (Select top 1000)** открывает
результат, **New SQL Console** создаёт вкладку с Rope-редактором и SQL-подсветкой.
Выберите соединение и схему, нажмите **▶ Execute** или **Ctrl/Cmd+Enter**.
Выполняется выделенный SQL либо выражение под курсором; Ctrl/Cmd+A позволяет
выполнить весь скрипт. Кавычки, dollar-quoted строки и комментарии защищают
вложенные `;` при выборе выражения. Схема применяется на том же SQL-соединении,
что и запрос (PostgreSQL search_path / MySQL USE); для SQLite используйте
квалифицированные `schema.table` имена, как в генерируемом preview.

Каждый набор результатов получает свою вкладку внизу консоли. Data Grid рисует
только видимые строки и колонки; заголовки содержат имя/тип. Клик по заголовку
переключает Asc/Desc/None (сортировка загруженных строк). Клик выделяет ячейку,
Ctrl/Cmd+C копирует значение; контекстное меню копирует строку CSV/TSV с
экранированием. Нижняя часть показывает число строк, elapsed ms и affected rows.
**Cancel** отменяет ожидание запроса; уже закоммиченные изменения могут остаться.

Выдача ограничена 10 000 строками / 16 МиБ сохранённых значений с пометкой
TRUNCATED. Результаты только для чтения; постраничная подгрузка, редактирование
ячеек, отдельные постоянные SQL-сессии/транзакции на вкладку и SQL LSP не входят
в эту версию. Транзакционный скрипт выполняйте одним запуском. SQLite JSON остаётся
Text, поскольку движок хранит его как TEXT; нативные PostgreSQL/MySQL JSON
возвращаются как Json. Неподдержанные vendor-типы дают ошибку декодирования.


## RC1: глобальное меню и единый интерфейс

File, Edit, View, Navigate, Code, Git, Database, Terminal, Tools и Help вызывают
зарегистрированные действия ActionRegistry. macOS использует нативное меню
AppKit через muda; Windows/Linux — встроенную строку меню egui. Подписи сочетаний
и нативные accelerators следуют текущему keymap, включая hot reload.
File → Open File / Folder принимает абсолютный путь; New File создаёт новый
файл без перезаписи существующего. Edit поддерживает Undo/Redo, Cut/Copy/Paste,
литеральный поиск и Replace/Replace All (одна undo-транзакция).

Общие семантические цвета, ANSI base palette, радиусы и ширина разделителей
собраны в `crates/ui/src/theme.rs`. Перетаскивайте границы Project/Git/SSH,
Database, нижнего терминала и разделитель SQL editor/results.
Пропорции сохраняются фоновым worker в `layout.json` рядом с `settings.toml`
и восстанавливаются при запуске. Повреждённый файл сохраняется без перезаписи.

При выходе оркестратор останавливает LSP (shutdown → exit), PTY/ConPTY,
SQL-пулы через Pool::close, SSH-сессии и туннели, затем основной Tokio runtime.
Каждый шаг ожидания ограничен по времени; аварийный kill процесса не даёт
такой гарантии. Help → About Nozhik показывает RC1, базовый Git commit сборки,
версию rustc, ОС/архитектуру, egui/Glow и Tokio.

Для отдельной конфигурации, например при проверке релиза:

```sh
cargo run -p app -- --config /absolute/test/settings.toml /absolute/test/project
```

### Keymap Cheatsheet

`Primary` — Cmd на macOS, Ctrl на Windows/Linux. Таблица — штатный JetBrains
профиль; пользовательский keymap переопределяет сочетания, меню показывает актуальные.

| Действие | Сочетание |
| --- | --- |
| New File / Open File or Folder | Primary+N / Primary+O |
| Save / Save All / Close Tab | Primary+S / Primary+Shift+S / Primary+W |
| Settings / Exit | Primary+, / Primary+Q |
| Undo / Redo | Primary+Z / Primary+Shift+Z |
| Cut / Copy / Paste | Primary+X / Primary+C / Primary+V |
| Find / Replace | Primary+F / Primary+R |
| Search Everywhere | Double Shift / Primary+Shift+A |
| Go to File / Recent Files | Primary+Shift+O / Primary+E |
| Git panel / Commit | Primary+Shift+G / Primary+K |
| New Terminal | Alt+F12 |
| Execute SQL | Primary+Enter |
| Git commit from Git panel | Primary+Enter |
| Go to Definition (PHP) | F12 / Primary+click |

UI больше не перерисовывается каждые 100 мс. Ввод, bus events и ответы workers
будят окно; Git debounce/poll и LSP requests назначают собственные сроки.
На macOS ARM64 измерено **0.6% одного CPU-ядра** за 30 секунд для пустого
non-Git проекта при заблокированном экране (0.18 CPU s / 30.01 wall s).
Это результат конкретного замера, не гарантия для любой ОС, GPU и нагрузки.
Стресс-тесты проверяют 10 МБ / 100 000 строк Rope, Data Grid 100 000×50 ячеек
с прокруткой обеих осей и PTY streaming + Git polling + LSP одновременно.
Проверка FPS на реальных дисплеях остаётся частью ручной release-матрицы.

RC1 обозначает полноту описанного набора Editor/Git/PHP LSP/Terminal/SSH/SQL.
Runner/debugger, marketplace, SQL editing/pagination и ограничения предыдущих
разделов сохраняются. Подробная матрица допуска релиза:
[verification checklist](docs/ai/verification-checklist.md).
