# Стандарты кода и правила для AI

## Перед изменениями

Читайте README, architecture.md и roadmap-progress.md. Проверяйте актуальные
Cargo.toml и публичные API. Реализуйте текущую фазу; не создавайте LSP,
рендерер, плагины и SQL-клиент без отдельной задачи. После изменения контрактов
обновляйте документацию и соответствующие поведенческие тесты.

## Rust и линтинг

- Текущий stable, edition 2024, resolver 3, rustfmt по умолчанию.
- Все пакеты наследуют workspace lints и общие версии зависимостей.
- clippy::pedantic и clippy::nursery включены; CI использует `-D warnings`.
- unsafe запрещён. `unwrap()` и `expect()` запрещены, включая тесты.
- Не отключайте группы lint целиком. Точечный allow требует причины в коде.
- snake_case для функций и модулей, PascalCase для типов, SCREAMING_SNAKE_CASE
  для констант. Пакеты именуются `nozhik-*`, imports — `nozhik_*`.
- Документируйте публичный API, единицы измерения и инварианты. Для fallible
  функций добавляйте `# Errors`; для возвращаемых значений используйте
  `#[must_use]`, когда игнорирование означает ошибку использования.

## Ошибки

Библиотеки возвращают конкретные ошибки thiserror, сохраняют source через
`#[from]` или `#[source]`. anyhow и context предназначены для composition root.
Не используйте panic, todo, unimplemented и индексацию без проверки для
обрабатываемых ошибок. В тестах допустимы assert, а fallible операции используют
Result и `?`. Не игнорируйте Result через `let _ =` без обоснованного контракта.
`unwrap_or` допустим для обычного fallback, когда его семантика документирована.

Отсутствующий файл настроек — штатный случай. Ошибки прав доступа, синтаксиса
и валидации не превращаются в defaults. Никогда не перезаписывайте повреждённый
пользовательский файл автоматически. При ошибке reload сохраняйте старое состояние.

## Зависимости и границы

- core: только модели, serde и ошибки. Без tokio, UI и файловой системы.
- event_bus: core, tokio, thiserror. Без config и app.
- buffer: core, event_bus, ropey, regex (экранирование литералов), regex-cursor
  (поиск по чанкам), thiserror. Без UI, прямого tokio и файлового I/O.
- config: core, event_bus, serde, toml, serde_json, directories, tempfile,
  thiserror. Без UI.
- fs_model: event_bus, vcs (read-only ignore/membership snapshots), notify, ignore, fuzzy-matcher, ropey, tempfile, thiserror.
  Без UI; синхронные операции вызываются выделенным worker.
- vcs: git2 (vendored libgit2/OpenSSL), thiserror, zeroize, tempfile (clone staging). Без UI/runtime/event_bus;
  Repository принадлежит единственному worker. Git index — источник staged-состояния.
- plugin_host: event_bus, tokio process/io-util, serde/serde_json, semver,
  directories, thiserror. Без egui, buffer и прямой зависимости от config.
- language_support: tree-sitter/php, lsp-types, tokio, ropey, serde_json, thiserror.
  Без UI; parser и LSP Child принадлежат фоновым workers, координаты LSP UTF-16.
- terminal: portable-pty, vt100 (vte), parking_lot, thiserror, Unix-only nix signals.
  Без UI/runtime/event_bus; I/O/ANSI parsing — только workers, UI получает Arc grid.
- remote_ssh: russh/keys, russh-sftp, tokio, keyring (native OS stores), zeroize,
  serde/serde_json, tempfile, directories, thiserror.
  Без UI/terminal/config; перед auth обязателен доверенный host key (TOFU с явным
  подтверждением либо legacy pin), секреты вне serde-профилей. Сохранение пароля
  только явным opt-in в OS credential store, без plaintext fallback; native API
  через spawn_blocking. Тесты используют injected/mock store без реального Keychain.
- database: sqlx (PostgreSQL/MySQL/SQLite, Tokio/Rustls, chrono/uuid/JSON/decimal),
  remote_ssh, tokio/tokio-util, futures-util, keyring, serde/serde_json, tempfile,
  thiserror. Без UI/config/event_bus; OS credentials только spawn_blocking,
  SSH только через проверенный SshSession, query/result limits и cancellation.
- ui: serde/serde_json/tempfile для фонового layout I/O; macOS-only muda для
  native menu (safe wrapper, main thread); safe objc2 AppKit для NSOpenPanel на
  macOS, rfd native/xdg-portal для picker на Windows/Linux. Project history JSON
  читается/записывается только последовательным worker с атомарной заменой. menu_bar каталог общий для реестра,
  embedded/native menus. Нет unconditional repaint; задавайте срок конкретной
  задачи либо callback wake. Close должен проходить dirty/revision gate.
- ui: database, buffer, core, config, event_bus, fs_model, vcs, plugin_host, language_support, terminal, remote_ssh, egui/eframe.
  remote_shell — адаптер ExternalTransport; прямые russh channel messages принадлежат этому адаптеру.
- app: собирает системы, владеет runtime и окном, добавляет контекст ошибок.
- Новая зависимость должна иметь конкретную необходимость и минимальные
  features. Обновляйте Cargo.lock. Изменения допустимых границ документируйте.

Не вводите глобальные изменяемые singleton, service locator, циклические
зависимости, универсальный Any-based event bus или преждевременные трейты.
Не заменяйте типизированные BufferId/Span/ConfigKey строками или числами.
Не используйте строковые пути и конкатенацию разделителей вместо Path/PathBuf.

## Конкурентность

Не держите std lock через await. Берите snapshot и освобождайте lock.
Не вызывайте UI и медленный I/O из async worker без spawn_blocking.
Главный поток принадлежит winit. Фоновые UI-уведомления должны будить event loop.
Проверяйте Lagged и Closed у broadcast receiver; не обещайте гарантированную
доставку. Для обязательных операций нужен отдельный command/request контракт.
replace и save вызываются без внешнего settings guard.

## Проверка перед сдачей

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release
```

Тестируйте наблюдаемое поведение: fan-out, lag/close, defaults, обе сериализации,
сохранение поверх файла, неверные настройки, события после commit. Используйте
временные каталоги; не меняйте реальные user settings и переменные окружения
процесса. Для проверок, способных зависнуть, задавайте timeout. Тесты не требуют GUI.
Ручной запуск окна проверяйте отдельно на каждой целевой ОС. Всегда указывайте,
что действительно запускалось, и не объявляйте CI успешным без результатов.


### Code Styles

`language_support` может зависеть от `config` для типизированного JSON CodeStyle;
обратная зависимость запрещена. Каталоги профилей и settings persistence читаются
и записываются только worker. Local formatter сохраняет синтаксические токены,
проверяет AST после whitespace edits и не пытается исправлять malformed source.
Ответы форматтера применяются только к исходным BufferId/revision и профилю,
одной транзакцией; range formatting не меняет текст за пределами selection.

### AI completion

- ai_completion: reqwest (JSON/stream/Rustls), tokio, serde/serde_json, thiserror, Unix-only nix for owned process groups;
  no UI, buffer, config or filesystem dependency. ui composes it with persisted
  config fields and the runtime supplied by app.
- Generation/debounce and model downloads must remain cancellable and off the
  UI thread. No implicit model download or polling repaint. Service launch is authorized
  when AI is enabled; only owned child processes may be terminated.

### Project symbols

language_support may use ignore for worker-only project traversal and tempfile
for atomic cache replacement. A file edit/save must never reparse other project
files. Retain parsed reference candidates for relinking; update usage counters
only for affected identities. Keep unsaved overlay facts out of persistent caches.
UI receives immutable index snapshots with source revision gates; symbol/cache I/O
and Tree-sitter extraction belong exclusively to the index worker.

Language IDs, extensions, availability and enabled-language policy belong to
config::languages; do not add independent extension allowlists to editor/worker
paths. A backend becomes available only after its actual service/extractor is wired.
Disabled languages must not start parsers/LSP/catalog loading or scan/read caches.
Keep filename search independent from semantic indexing. Batch startup linking and
skip snapshot/cache writes for no-op and unsupported-file notifications.

Git ignore rules affect tree coloring only. Semantic source discovery and filename
search include ignored/vendor files; semantic contents still require an enabled,
implemented language. Skip only .git/.nozhik internals and do not follow symlinks.
Tracked files and directories containing them cannot be painted ignored just because
an ancestor matches .gitignore. Filesystem workers may call vcs read-only membership
classification; repository/index handles must stay local to that call and never write.
Keep deferred receiver type facts bounded and retain their dependencies for relinking;
navigation and reverse usages must share resolved edges. PHPDoc type tags are
references; prose is not.

### Launcher / GitHub

UI may use reqwest for session-only GitHub REST access on a dedicated worker.
Never serialize clone credentials or embed secrets in remote URLs/errors. Explicit
Git credentials must reach libgit2 callbacks; certificate/host-key verification
must stay enabled. Cancelled completions must not open projects. Version follows
workspace.package; increase patch once per follow-up change/commit (see AGENTS.md).


### Лицензии

Собственные пакеты наследуют workspace.package.license-file (LICENSE).
Не добавляйте Apache/MIT headers к собственному коду Nozhik. Сторонние notices
не удаляйте и не перелицензируйте. Для новых зависимостей, ресурсов и данных
проверяйте условия использования в платной Source-Available IDE. После изменений
обновляйте legal sidecars через update-binaries.py; MPL source и upstream notices
должны оставаться в поставке и Resources/Legal. При добавлении нового legal
build input включайте его в fingerprint и пути CI.
