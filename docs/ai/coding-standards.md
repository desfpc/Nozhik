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
- fs_model: event_bus, notify, ignore, fuzzy-matcher, ropey, tempfile, thiserror.
  Без UI; синхронные операции вызываются выделенным worker.
- vcs: git2 (vendored libgit2/OpenSSL), thiserror. Без UI/runtime/event_bus;
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
- ui: buffer, core, config, event_bus, fs_model, vcs, plugin_host, language_support, terminal, remote_ssh, egui/eframe.
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

