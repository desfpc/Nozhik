# Roadmap и прогресс

## Иконка приложения (2026-10-26)

Из предоставленной владельцем WebP подготовлены прозрачные PNG/ICO/ICNS
с сохранением пропорций и квадратными полями. app задаёт встроенную viewport
icon; Windows build.rs добавляет семь размеров ICO в PE resources через
MinGW windres или Windows SDK rc.exe. Новых Rust-зависимостей нет.
update-binaries.py дополнительно создаёт Nozhik.app.zip с Info.plist, ICNS
и локальной ad-hoc подписью полного bundle; Developer ID/notarization нет.

Проверки: fmt, Clippy workspace/all-targets/all-features с -D warnings для
macOS ARM64 и Windows GNU, полный workspace test (102 passed, 4 прежних ignored),
workspace release build и git diff --check прошли. Обе платформы пересобраны;
--check подтвердил source fingerprint и SHA-256 payload. Проверены Windows
ICON/GROUP_ICON resources, ZIP integrity, plist, ICNS и executable permissions,
codesign --verify --deep --strict после распаковки архива.
macOS .app успешно запущен через LaunchServices вне sandbox на временном
проекте с отдельными settings; окно визуально проверено и закрыто Cmd+Q.
Снимок Dock через UI automation получить не удалось (timeout), поэтому
отображение иконки в Dock визуально не подтверждено. Windows GUI/MSVC build
не запускались; runtime_tested=false сохранён для неполной OS-матрицы.

## Общий план

Этапы 1–9 — предварительная последовательность, не обещание готовых функций.

| Этап | Объём | Статус |
| --- | --- | --- |
| 0 | Каркас, документация, типы, шина, настройки, окно, CI | Реализован; проверки см. ниже |
| 1 | Текстовое ядро: Rope, версии, курсоры, транзакции, история, поиск | Реализован |
| 2 | UI Shell & Windowing: egui, панели, редактор, поиск | Реализован; ограничения ниже |
| 3 | Проекты, файловый I/O, файловое дерево, watcher, поиск | Реализован; ограничения ниже |
| 3.4 | VCS Engine & GitHub Desktop UI | Реализован; ограничения ниже |
| 4 | Plugin & Action Architecture: manifests, process host, actions, keymaps | Реализован; ограничения ниже |
| 4.1 | Tree-sitter, подсветка, синтаксические индексы | Подсветка включена в этап 5; индексы отложены |
| 5 | Tree-sitter Syntax Highlighting & LSP Client Core (PHP) | Реализован; рефакторинг отложен |
| 6 | Terminal Subsystem: PTY, ANSI grid, вкладки, TUI | Реализован; ограничения ниже |
| 6.1 | БД: подключения, схема, SQL-консоль, результаты | Реализован в этапе 8 |
| 7 | Remote Development & SSH: профили, remote shell, SFTP, local forwarding | Реализован; ограничения ниже |
| 7.1 | Task runner, запуск и отладка | Не начат |
| 8 | Database Tooling: PostgreSQL/MySQL/SQLite, SSH, Explorer, SQL, Data Grid | Реализован; ограничения ниже |
| 8.1 | Развитие API расширений, темы, marketplace | Фундамент плагинов/keymap вынесен в этап 4 |
| 9 | Финальная интеграция, global menus/actions, layout/theme, shutdown, stress/idle | Реализован; v0.1.0-RC1 (Feature Complete) |
| 9.1 | Полная ручная OS-матрица, accessibility/security audit, подписанные пакеты | Release gates; см. verification-checklist.md |

## Phase 0 — реализовано

- [x] Workspace из core, event_bus, config, app, edition 2024.
- [x] Общие зависимости, строгие lints, запрет unsafe/unwrap/expect.
- [x] BufferId, Position, валидируемый Span и ConfigKey.
- [x] Ограниченная асинхронная pub/sub шина, Arc payload, Send + Sync.
- [x] FileOpened, BufferModified, ConfigChanged, ActionTriggered.
- [x] Settings из user config directory, TOML/JSON, defaults по отсутствию.
- [x] Валидация, snapshot, replace/reload, Arc<RwLock<Config>>.
- [x] Явный save через временный файл; уведомление после замены в памяти.
- [x] Базовое winit-окно и Tokio consumer с ограниченным shutdown.
- [x] Тесты контрактов и workflow для Windows/macOS/Linux.
- [x] README и база знаний AI.

## Зафиксированные контракты

1. core ничего не знает о runtime и UI; app — composition root.
2. UTF-8 байтовые колонки, zero-based позиции, полуинтервал Span.
3. Bus — best-effort уведомления без replay, ack и backpressure.
4. Подписчик явно обрабатывает отставание и закрытие канала.
5. ConfigChanged — invalidation; manager пока публикует только All.
6. Ошибочный файл возвращает ошибку; только NotFound означает полный fallback.
7. replace не сохраняет файл, save не публикует ConfigChanged.
8. GUI работает на главном потоке; I/O после старта выполняется в workers.

## Верификация

Локально на macOS arm64, Rust 1.99.1 stable, после реализации Phase 1:
Clippy с pedantic/nursery и `-D warnings` прошёл; все 18 unit-тестов
(13 в buffer) и один doc-test прошли. `cargo fmt --all -- --check`,
`git diff --check` и release-сборка workspace также прошли.
Тест на 50 000 строк проверяет разделение чанков снимком, локальность правки,
поиск и undo с широким лимитом 30 секунд. Это регрессионная проверка,
а не измерение RSS/аллокаций или гарантия задержки на всех платформах.
Матрица CI предусмотрена для трёх ОС. Ручное открытие окна и проверка shutdown
на Windows, macOS и Linux остаются отдельными платформенными проверками.

## Phase 1 — текстовое ядро реализовано

- [x] Отдельный `nozhik-buffer`, Ropey и заимствованные срезы viewport.
- [x] Position: line, byte column, абсолютное byte_offset; проверка UTF-8.
- [x] Направленные Selection/Cursor, CursorSet со слиянием и пересчётом после правок.
- [x] Атомарные TextEdit/EditTransaction, применение справа налево.
- [x] History: undo/redo, явные группы, восстановление курсоров, ветвление.
- [x] Точка сохранения и is_dirty относительно идентичности состояния.
- [x] Монотонная revision, дешёвый snapshot, события BufferEvent.
- [x] Литералы и Regex по чанкам без полной копии текста.
- [x] Тесты Unicode, ошибок/атомарности, групп, dirty, событий, курсоров,
  многострочного поиска и 50 000 строк с проверкой разделяемого хранения.

### Публичный API для следующих модулей

`TextBuffer::load_from_str(BufferId, &str)` создаёт чистый буфер с одним курсором.
ID выделяет вызывающий код, повторное использование запрещено в рамках будущего
реестра документов. Файловый I/O и BufferStore вынесены в следующую интеграцию;
объём текущего этапа уточнён до независимого текстового ядра.

| API | Контракт |
| --- | --- |
| `insert(offset, text)`, `delete(range)` | Байтовые UTF-8 границы, одна операция истории |
| `apply_transaction(EditTransaction)` | Диапазоны в исходной версии; пересечения запрещены; ошибка атомарна |
| `EditTransaction { edits, group }` | `Some(id)` группирует последовательные транзакции с одинаковым ID |
| `undo()`, `redo()` | Целая группа, с восстановлением курсоров и событиями |
| `take_history()` | Явное владение History для `undo/redo(&mut TextBuffer)`; проверяется владелец/состояние |
| `mark_saved()`, `is_dirty()` | Отметить текущее успешно сохранённое состояние; закрывает группу |
| `position_at(byte)`, `char_to_position(char_idx)` | Полная Position; char_idx измеряется в Unicode scalars |
| `line_col_to_char(line, byte_col)` | Обратная конверсия без перескока на следующую строку |
| `line(index)`, `lines(start..end)` | Заимствованные RopeSlice с исходными line endings |
| `rope()` | Только чтение; мутации проходят через транзакции |
| `set_cursors(Vec<Cursor>)`, `cursors()` | Проверка координат, слияние; пустой набор запрещён |
| `snapshot()` | BufferId, revision, Rope COW и CursorSet; авторитетный resync |
| `set_event_bus(bus)` | События после commit; без runtime и гарантии доставки |
| `SearchEngine::literal/regex(pattern)?.find_all(&buffer)` | Vec<Range<usize>>, UTF-8 байты, включая multiline и пустые совпадения |

Modified содержит список последовательных замен, CursorChanged — канонический
набор выделений после commit. На Lagged нужно запросить snapshot у владельца.
UI должен самостоятельно реализовать графемную/экранную навигацию, применение
команд ввода и таймер, который выбирает ID группы. История пока без лимита памяти.

## Phase 2 — UI Shell & Windowing

- [x] `crates/ui` (`nozhik-ui`), egui/eframe 0.32.3, Glow/OpenGL, X11/Wayland.
- [x] `app`: `eframe::run_native` на главном потоке, Tokio consumer событий.
- [x] New UI Dark: точные HEX-цвета в theme.rs; toolbar, toolstrips,
  изменяемые размеры левого/правого/нижнего dock, status bar.
- [x] Вкладки с close и dirty; закрытые sample-буферы сохраняются до выхода.
- [x] TextEditorView: show_rows виртуализирует строки Rope, gutter, каретка,
  клик, Shift-выделение, Unicode scalar navigation, вставка, удаление, undo/redo.
- [x] Мутации через EditTransaction; TextBuffer публикует Modified/CursorChanged.
- [x] Search Everywhere: Double Shift / Ctrl/Cmd+Shift+A, модальный overlay,
  chips, query, список, стрелки, Enter, Esc; настройки — отдельное modal.

Открытие модальных окон: `bus.publish(Event::ActionTriggered(
nozhik_ui::SEARCH_ACTION.into()))` или `SETTINGS_ACTION`. UI подписывается
в конструкторе, читает события в update без блокировки. Repaint каждые 100 мс
ограничивает задержку внешних запросов. Это best-effort broadcast: подписчик
должен существовать до публикации, доставка не гарантируется. Lagged выводится
в статус, настройки перечитываются; редактор читает авторитетные буферы напрямую.
Настройки modal меняются через ConfigManager::replace и ConfigChanged,
без неявной записи пользовательского файла.

Исторические границы Phase 2 (заменены Phase 3): демонстрационное дерево и in-memory документы; реального VFS,
сохранения, Git, runner/debugger, terminal, DB нет. Classes/Symbols и checkbox
внешних результатов — UI-каркас до появления индекса. Нет IME preedit,
графемной навигации, syntax highlighting, drag docking и персистентности layout.
Строки виртуализируются; одна очень длинная строка layout-ится целиком.
Ручная проверка окна на каждой из трёх ОС остаётся отдельной задачей.

Проверки этой итерации (macOS arm64): `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo test --workspace --all-features` и `cargo build --workspace --release`
успешны. Четыре UI-теста покрывают Unicode-ввод/историю/события, удаление
CRLF/выделения, открытие modal через bus и Double Shift, пустой workspace,
стрелки/Enter/Esc в поиске. Тесты egui выполняются без системного окна;
визуальная проверка нативного окна на macOS/Windows/Linux не выполнялась.

## Phase 3 — файловая система и Project Model

- [x] `nozhik-fs-model` в workspace: `notify`, `ignore`, `fuzzy-matcher`, Rope snapshot I/O.
- [x] `tree.rs`: `ProjectTree { root_path, directories }`, `DirNode { path, children }`,
  `Node::Dir(PathBuf)` / `Node::File(FileNode)`, типы Rust/PHP/JS/TS/config/Markdown/generic.
  `children: None` означает отсутствие загрузки; `Some([])` — загруженный пустой каталог.
  Загружается только один уровень; папки сверху, обе группы по имени.
- [x] `fuzzy.rs`: отдельный плоский `FileIndex`, обход не раскрывает lazy-дерево;
  fuzzy-рейтинг относительных путей, детерминированная сортировка, максимум 100 результатов в UI.
- [x] Единая ignore-политика: `.gitignore`, вложенные/родительские правила, `.ignore`,
  глобальные Git rules; `.git`, `target`, `node_modules`, `vendor` исключаются всегда.
- [x] `watcher.rs`: recommended OS watcher, отдельный debounce worker, 150 мс тишины
  на путь; access events пропускаются, ignore matchers перечитываются после изменений.
- [x] `lib.rs`: `FileSystem` с выделенным serial worker и каналами `Command` / `Reply`;
  чтение, запись, обход, поиск, canonicalize и настройка watcher выполняются вне egui.
- [x] UI: реальное дерево, двойной клик/Enter, дедупликация по canonical path,
  уникальные BufferId, сохранение по Ctrl/Cmd+S и Files в Search Everywhere.
- [x] Сохранение: COW Rope snapshot + revision; serial worker сохраняет порядок,
  temp file + sync_all + persist, исходные permissions; dirty снимается после успеха
  только при совпадении текущей ревизии. Ошибки не меняют точку сохранения.
- [x] NUL в первых 1024 байтах → binary placeholder без editing/save;
  невалидный UTF-8 и I/O errors → status bar + stderr, без panic/unwrap.
- [x] CLI project path или текущая директория; поле пути + Open для смены проекта.

### Контракты событий и конкуренции

`Event::File(FileEvent::Changed(PathBuf))` — абсолютный native path изменившегося
файла или каталога; включает create, remove и обе стороны rename. Это invalidation,
а не точный diff и не гарантия внешнего происхождения: собственные записи тоже видны.
`Event::File(FileEvent::Error(String))` сообщает ошибку watcher; проект остаётся
доступным для чтения/сохранения, полнота автоматического обновления не гарантируется.
`Event::FileOpened(PathBuf)` публикует UI после установки документа/placeholder.

Обязательные операции не идут через best-effort bus. `Command`: OpenProject,
LoadDir, Refresh, Read, Save { path, revision, text: Rope }, Search { token, query }.
`Reply`: Tree, Refreshed, Read, Binary, Saved, Matches, Error. Завершения приходят
по отдельному mpsc; egui читает их без ожидания. Дерево обновляется по Changed,
на Lagged запрашивается Refresh. Изменения во время обновления планируют повторный
Refresh. UI не меняется из worker; callback вызывает `Context::request_repaint()` после
завершения команды или уведомления watcher. Дополнительный repaint с интервалом
100 мс ограничивает задержку внешних bus-действий. Search token отбрасывает ответы устаревших запросов.

Для I/O используются `PathBuf` и canonical paths; для поиска — относительные пути
с `/`, запросы также принимают `\`. UI хранит свои буферы авторитетно; закрытые
вкладки и смена проекта их не уничтожают. Watcher обновляет дерево/индекс, но
не заменяет текст открытого буфера. Нет автоматического reload/merge, file locking,
восстановления сессии или запроса о сохранении при выходе. Запись атомарно заменяет
файл, но не гарантирует durability каталога при аварийном отключении; ACL/xattrs
не копируются, hard links не сохраняют общую файловую идентичность после замены.
Ссылки при обходе не раскрываются. Очень большие файлы и длинные строки требуют
дальнейших лимитов/оптимизации. Фоновый worker последовательный: полный scan
может задерживать I/O/search, но не рендеринг UI.

### Проверки Phase 3

Поведенческие тесты покрывают lazy/sorted tree, ignore rules и вложенные исключения,
fuzzy path search и Windows-разделители запросов, чтение/UTF-8/binary/errors,
фоновый индекс и refresh, сохранение/permissions, дедупликацию вкладок,
правку во время save, Ctrl/Cmd+S, удалённый файл, binary placeholder,
Files Enter и отклонение устаревшего search token. Нативные окна Windows/Linux
нужно проверять отдельно; тесты egui выполняются без окна.

Локально на macOS arm64, Rust 1.99.1: fmt, Clippy workspace/all-targets/all-features
с `-D warnings`, 31 unit-тест и один doc-test прошли. Реальный OS watcher
проверен вне sandbox: внутри sandbox FSEvents не доставлял события.
Release-сборка успешна; для временного toolchain использован `DYLD_LIBRARY_PATH`
к его lib-каталогу, чтобы rust-objcopy находил libLLVM при stripping.
Нативное GUI-окно и Windows/Linux в этой итерации вручную не проверялись.

## Phase 3.4 — VCS Engine & GitHub Desktop UI

- [x] `nozhik-vcs` в workspace, `git2` 0.21 с https/ssh и vendored libgit2/OpenSSL.
- [x] `model.rs`: owned `GitRepoStatus`, `FileChange`, `ChangeStatus`, `FileDiff`,
  `DiffHunk`, `DiffLine`, `DiffLineKind`, `CommitInfo`, `DiffTarget`.
- [x] `lib.rs`: выделенный serial worker, `Task` / `Reply` / `Outcome`,
  typed `VcsError`, корреляция по project generation и request ID, wake callback.
- [x] `git_worker.rs`: discovery, index/worktree status и rename detection,
  upstream/ahead/behind, staged/combined/historical diff, commit/history/details,
  stage/unstage, локальные ветки, fetch/push, fast-forward pull, discard/ignore.
- [x] UI `vcs_panel.rs`: Changes/History, Filter, Select All и staging,
  required Summary/Description/commit box, контекстные discard/ignore.
- [x] `diff_view.rs`: виртуализированный unified diff, заголовки hunks,
  два gutter, типы строк и цветные фоны, +added/−deleted.
- [x] History: виртуализированные карточки, автор и относительное время,
  SHA/copy, тело сообщения, affected files и historical diff первого родителя.
- [x] Git toolbar: текущая ветка в toolbar/status/title, поиск локальных веток,
  New Branch/checkout, Fetch/Push origin, меню Pull/Refresh, ahead/behind и spinner.
- [x] Ctrl/Cmd+Shift+G переключает Git; Ctrl/Cmd+Enter коммитит из Git-панели.
- [x] Сохранение и FileEvent/lag инвалидируют Git status с debounce 200 мс;
  опрос раз в 3 секунды у открытого repo покрывает исключённые watcher `.git`-пути.

### Публичный API и конкурентность

`Vcs::start()` создаёт канал и worker, не читая диск на вызывающем потоке.
`set_wakeup(Fn + Send + Sync)` однократно регистрирует пробуждение event loop.
`send(generation, id, Task) -> bool` неблокирующе ставит команду в mpsc;
`drain()` читает до 128 завершений без ожидания. `Reply { generation, id,
result: Result<Outcome, VcsError> }` относится к одной команде. Репозиторий
никогда не покидает worker. Drop handle закрывает очередь; worker завершает
текущую операцию и прекращает работу при недоступности receiver.

| Task | Outcome |
| --- | --- |
| Open(PathBuf), Refresh | Status(Option<GitRepoStatus>) |
| Stage { paths, staged } | Done |
| Diff(DiffTarget) | Diff(FileDiff) |
| History(limit) | History(Vec<CommitInfo>) |
| Details(sha) | Details { commit, files } |
| Commit { summary, body } | Committed(CommitInfo) |
| Fetch, Push, Pull, Checkout(name), NewBranch(name), Discard(path), Ignore(path) | Done |

Open выполняет discovery вверх от папки проекта; не-repo/bare — нормальный
Status(None). Root status — canonical worktree root. Native пути repository-relative;
отображение не определяет идентичность. `is_staged` означает отличие index от HEAD,
`has_unstaged` — отличие worktree от index, оба флага могут быть true. Источники
rename учитываются отдельно: staging rename не добавляет воскресший старый путь.
Unstage восстанавливает точные entries index из HEAD, а не wildcard pathspec.
Commit записывает только index, проверяет summary, конфликты и отсутствие активного
merge/rebase; не вызывает неявный stage. После успеха UI очищает поля и перечитывает
status/history; оставшиеся unstaged файлы не скрываются.

UI отбрасывает результаты старой generation после смены проекта, а diff/details
сверяет по актуальному request ID и выбранному режиму. Переход History → Changes
не допускает поздний historical diff. FileEvent от fs_model и save completion
вызывают invalidate; это не Git-команды через best-effort broadcast. На ответ worker
вызывает `Context::request_repaint`, egui не ждёт I/O и не держит Repository.

### Безопасность и ограничения

Discard требует явного UI-подтверждения. Обрабатывается только точный выбранный
путь (для rename — оба пути), запрещены traversal/absolute/.git пути и выход
через родительскую symlink-директорию. Checkout pathspec matching отключён для
Discard. Удалённый родительский каталог может быть создан при восстановлении
tracked-файла; занятый старый путь rename защищён от перезаписи. Ignore экранирует
Git pattern metacharacters и не пишет через symlink `.gitignore`.

Переключение веток и fast-forward Pull требуют чистого repo; игнорируемые локальные
файлы не перезаписываются checkout. Divergence/rejection/credentials/locks возвращают
ошибку в UI/stderr. Только origin и локальные ветки; push без force, credentials —
SSH-agent/HTTPS helper, без хранения секретов и ослабления проверки сертификатов.
Удалённые сервисы в тестах не используются: network workflow проверяется на локальном
bare origin и втором clone. Сетевые задачи не имеют UI cancellation; очередь Git
последовательная, поэтому долгая сеть задержит новые Git-ответы, но не рендеринг.

Diff: HEAD→index, HEAD→worktree with index, первый parent→commit; для первого commit
старое дерево пустое. Binary/oversized показывается отдельно; лимиты 2 MiB,
50 000 строк, 4096 Unicode scalars/строку. Выдача ограничена, это не механизм
построчного staging. History до 10 000 записей с загрузкой по 100. Идентичность
берётся из Git config; hooks/GPG/merge UI/rebase/submodule lifecycle не реализованы.
Буферы редактора авторитетны для несохранённого текста и автоматически не reload-ятся
после Git-операций. Git status/commit/diff работают с диском/index, не с этими буферами.

### Проверки Phase 3.4

Локально на macOS arm64, Rust 1.99.1: `cargo fmt --all -- --check`,
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`,
`cargo test --locked --workspace --all-features` (39 unit-тестов и один doc-test),
`cargo build --locked --workspace --release`, `git diff --check` прошли.
Существующий тест FSEvents выполнялся вне sandbox; Git-тесты работают с временными
репозиториями и локальным bare origin без обращения к внешним remotes.

Семь VCS-тестов покрывают status/unborn, index-only commit при более новых
worktree-правках, staged/unstaged, unified/historical/binary diff, rename/deletion,
точные пути с wildcard-символами, удалённые parent directories, защиту воскресшего
источника rename, branch/create/dirty guard, escape в ignore patterns, fetch/push/
fast-forward/ahead/behind/divergence и корреляцию каналов/wake callback.
UI-тест проходит staging → commit → history → historical diff и рендер обеих
панелей без окна, проверяет отклонение устаревшего Details после смены режима.
Тест shortcuts проверяет Ctrl/Cmd+Shift+G и открытие Git-панели.
После финальной правки shortcut/modal guards UI-тесты и Clippy повторно прошли.

Нативное GUI-окно, реальные HTTPS/SSH credentials и Windows/Linux вручную
не проверялись; CI-матрица предусмотрена, но её прогон этой итерацией не заявляется.
Для временного macOS toolchain при release используется `DYLD_LIBRARY_PATH`
на его lib-каталог для загрузки LLVM rust-objcopy, как в Phase 3.

## Phase 4 — Plugin & Action Architecture

- [x] Девятый workspace crate `nozhik-plugin-host`; serde/serde_json, semver,
  Tokio process/io-util, отдельный host worker без зависимости от egui.
- [x] Строгий `PluginManifest::parse`: metadata, activation events, languages,
  commands/when, typed configuration defaults, tabs, executable/args/dependencies.
- [x] Scan глобального/локального каталогов; минимум IDE, диапазоны версий
  зависимостей, циклы и коллизии; ошибки отдельных установок не прерывают scan.
- [x] Enabled/Disabled/Active/Failed, активация FileOpened по расширению/языку,
  автоматическая активация команды, включение зависимостей, каскад Disable.
- [x] JSON-RPC/NDJSON initialize/executeCommand, bounded queue, 3s timeout,
  64 KiB response limit, проверка ID/ошибок, kill/reap процесса, отмена при shutdown.
- [x] `ActionRegistry`: register, available, execute, unregister_owner;
  Arc/RwLock, событийный обработчик для built-ins, процессная очередь для плагинов.
- [x] `KeymapRegistry`: JetBrains/VSCode/Custom, атомарная замена профиля,
  последний эквивалентный chord побеждает, контексты и hot reload keymap.json.
- [x] Динамические Actions в Search Everywhere с shortcuts и Enter; Plugins
  в Settings/Actions, версии/авторы/статусы, Enable/Disable, scan, generic tabs.
- [x] Тесты манифестов, SemVer, dependency graph, collisions, cascade Disable,
  смены проекта, action → keypress → bus, reload с сохранением прежнего профиля,
  реального процесса (успех, crash, timeout, oversized response), UI Actions.

### Формат и API

Полная спецификация и примеры: [plugin-authoring-guide.md](plugin-authoring-guide.md).
Обязательные поля манифеста: id/name/version/min_ide_version/description/author/
activation_events; contributes содержит languages/commands/configuration/tabs.
Executable/args и dependencies дополняют декларативный контракт процессным.
SemVer min_ide_version — включительный минимум, dependencies — VersionReq.
Команды `ui.*` зарезервированы IDE. События: `*`, `onLanguage:id`,
`onFileExtension:.ext`, `onCommand:id`.

Регистрация: `manager.actions().register(Action { id, title, when, owner: None })`.
Вызов: `execute(id, ActionContext)`; built-in публикует ActionTriggered,
plugin-owned команда ставится в надёжную локальную очередь и публикует уведомление.
`when`: editorOpen/editorFocus/selectedFile/projectOpen с `&&`/`!`.
`manager.keymap().replace(KeymapConfig)` меняет профиль; `dispatch` разрешает
нажатие с проверкой контекста; UI читает registry для поиска и shortcuts.
`manager.snapshot()` — авторитетный снимок статусов, ассоциаций, вкладок,
конфигурационных defaults и диагностики. FileOpened остаётся best-effort;
при пропуске уведомления повторное открытие файла повторяет активацию.

### Границы текущей версии

Это изоляция отдельным процессом с правами пользователя, без OS sandbox.
Host последовательно обслуживает RPC; неответивший процесс задерживает очередь
до таймаута, UI продолжает работать. Disable/shutdown завершают прямой процесс;
дочерними процессами управляет расширение. Toggle действует до scan/перезапуска;
scan включает валидные установки заново. Marketplace/installation API,
LSP/Tree-sitter/БД, callbacks в IDE, редактирование буфера через RPC,
произвольные plugin UI и пользовательские значения plugin configuration пока
не реализованы. Tabs — метаданные для generic view в Plugins; result RPC — ack.
Текстовые undo/redo и Git commit shortcuts пока принадлежат своим views.

### Верификация Phase 4 (2026-10-23)

Локально на macOS arm64 / stable Rust 1.99.1 прошли:
`cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --all-features --offline -- -D warnings`,
`cargo test --workspace --all-features --offline` (47 unit + 1 doc-test),
`cargo build --workspace --release --offline`, `git diff --check`.
Первый тестовый прогон внутри sandbox не получил события macOS watcher;
полный успешный прогон выполнен вне sandbox. Новый процессный тест компилирует
std-only fixture через rustc и проверяет настоящий IPC, аварийное завершение,
3s timeout, 64 KiB лимит и отмену ожидающей инициализации при Drop хоста.
UI тестируется headless через egui RawInput; ручной запуск нативного окна,
Windows/Linux и матрица CI в этой итерации не проверялись.


## Phase 5 — Tree-sitter Syntax Highlighting & LSP Client Core

- [x] Десятый крейт `nozhik-language-support`, Tree-sitter 0.25 / PHP 0.24,
  lsp-types 0.95 (структуры LSP 3.17), Tokio process/io-util; обновлён Cargo.lock.
- [x] `highlighter.rs`: bundled highlights.scm, минимальная contiguous InputEdit,
  повторное использование tree; фоновые query captures нормализуются в отсортированные
  непересекающиеся UTF-8 диапазоны. Viewport выбирает их бинарным поиском.
- [x] `lsp/client.rs`: Content-Length framing, UTF-8 byte length, fragmented reads,
  дополнительные headers, лимиты 8 KiB headers / 16 MiB body; response ID routing,
  concurrent requests, отдельные notifications, remote errors, EOF и 5s timeout.
- [x] `lsp/manager.rs`: discovery phpactor/intelephense, rootUri, initialize/initialized,
  capabilities UTF-16/no snippets, версии документов, didOpen/Change/Close,
  full/incremental sync, shutdown/exit и kill-on-drop.
- [x] Отдельные syntax thread и LSP actor с собственным current-thread runtime;
  отмена ожидающего RPC при Drop/смене проекта, bounded cleanup при выходе.
- [x] Встроенная языковая конфигурация `org.nozhik.php`: `.php` / `.phtml`,
  phpactor language-server, fallback intelephense --stdio, инструкция при отсутствии.
- [x] UI: Darcula syntax colors и italic comments, маркеры gutter, красные/жёлтые
  волнистые линии, сообщения diagnostics при наведении, counters status bar.
- [x] Completion с иконками/подписями/detail, prefix/filter/sort, стрелками,
  Enter/Tab/Esc, textEdit/insertReplace и additionalTextEdits одной транзакцией.
- [x] Hover с базовым Markdown; Ctrl/Cmd+клик/F12 → первый definition Location/Link,
  файловое открытие через fs_model и прокрутка к строке.

### Контракты и конкуренция

`LanguageService::start(root, wake)` не выполняет I/O на UI-потоке.
`with_server(root, Some(ServerCommand { program, args }), wake)` позволяет явно
выбрать executable (в том числе mock). UI передаёт Rope snapshots только при
смене ревизии. `send(Command) -> bool` использует две очереди ёмкостью 64;
при false UI повторяет snapshot/close. Частичная доставка между очередями
допускает idempotent повтор. Все I/O и parse происходят вне egui; completed events
передаются через mpsc, wake вызывает request_repaint. UI drain ограничен 128 events
за кадр. LSP actor последователен для editor requests; reader транспорта
независим и маршрутизирует параллельные request IDs/серверные уведомления.

| Команда | Результат / смысл |
| --- | --- |
| Sync { uri, revision, text: Rope } | didOpen для нового URI; incremental parse сразу, didChange после 200 ms покоя |
| Close(uri) | Удаление parser/document state, didClose; UI удаляет diagnostics |
| Request { uri, revision, token, position, kind } | Completion / Hover / Definition typed response; перед запросом flush актуального текста |
| Event::Highlights { uri, revision, spans } | Отсортированные nonoverlapping UTF-8 byte ranges + query token names |
| Event::Diagnostics { uri, revision, items } | Типизированные LSP diagnostics, версия сверяется при наличии |
| Event::Response { uri, revision, token, data } | Типизированный ответ исходного UI-запроса |
| Event::Error(String) | Ошибка сервера/парсера/установки в status и stderr |

Интерактивные completion/hover запросы также ждут 200 ms покоя; definition
выполняется сразу. UI проверяет document revision, отдельные request tokens и
completion caret, отбрасывая устаревшие результаты. При смене проекта заменяется
весь service/receiver. Закрытие вкладки сохраняет авторитетный TextBuffer, но
освобождает его языковое состояние; повторное открытие отправляет didOpen.

Byte offsets Buffer остаются UTF-8; `position`, `byte_offset`, `byte_range`
конвертируют LSP UTF-16 с проверкой surrogate boundaries, порядка range и CRLF.
LSP versions — отдельный i32 счётчик, UI revision — u64. Для incremental servers
didChange заменяет весь предыдущий документ одним range; это корректный LSP
incremental пакет, не минимальный diff. Серверные configuration requests получают
null defaults, workDoneProgress/create — ack, прочие requests — MethodNotFound.

Drop неблокирующе сигнализирует stop; actor отменяет текущую операцию и пытается
закрыть документы/shutdown/exit в пределах 2 секунд. Client kill_on_drop — fallback.
`eframe::App::on_exit` ждёт cleanup максимум 3 секунды; обычные кадры не ждут.

### Ограничения

Встроенная PHP-конфигурация отделена от plugin_host NDJSON RPC; её нельзя выключить
через переключатель executable-плагинов. Пользовательские language settings,
completion resolve/snippets/commands, code actions, rename/refactoring, semantic
index/tokens, injected HTML/CSS/JS грамматики, restart/retry упавшего сервера пока
не входят в этот этап. Hover поддерживает базовое подмножество Markdown.
Definition выбирает первый результат. Diagnostics без server version нельзя
строго сопоставить исходной ревизии: они относятся к последнему синхронизированному
снимку; versioned diagnostics проверяются строго. Лимитированные headers/body
не заменяют OS sandbox. Очень большие файлы всё ещё требуют performance budgets:
полный query pass и materialization строки остаются в worker, не в UI.

### Верификация Phase 5 (2026-10-26)

Новые behavioral tests: PHP query tokens и incremental/fresh equivalence с
Unicode; UTF-16/CRLF/invalid ranges; completion replacement/import edits;
framing с трёхбайтовым reader buffer, Unicode и склеенными пакетами,
missing/duplicate/oversized/truncated lengths. Std-only mock компилируется rustc
во временный executable и проверяет настоящий stdio initialize, virtual didOpen,
publishDiagnostics, параллельные completion/hover, document versions/close/shutdown.
Второй worker test проверяет coalescing трёх правок в один didChange после 200 ms,
правильный incremental range и shutdown/exit. Headless egui test рисует PHP syntax,
diagnostics и popup, затем выбирает/принимает completion стрелкой и Enter,
проверяет замену префикса без newline и undo.

Реальные phpactor/intelephense, нативное окно и Windows/Linux вручную не проверялись.

Финальная локальная проверка на macOS arm64 / Rust 1.99.1: прошли
`cargo fmt --all -- --check`,
`cargo clippy --locked --workspace --all-targets --all-features --offline -- -D warnings`,
`cargo test --locked --workspace --all-features --offline`
(48 unit + 5 integration + 1 doc-test),
`cargo build --locked --workspace --release --offline`, `git diff --check`.
Полный test run выполнен вне sandbox: внутри него существующий тест FSEvents
не получает файловые события. Проверки реальных LSP и платформенной CI-матрицы
этим прогоном не заявляются.


### Готовые бинарники в репозитории

По требованию владельца добавлены `binaries/macos-arm64/nozhik` (Mach-O ARM64)
и `binaries/windows-x64/nozhik.exe` (PE32+ x86-64, cross-build MinGW-w64 на macOS).
Обе release-сборки настоящие, привязаны к одинаковому fingerprint build inputs;
`build.json` хранит target/host/rustc и SHA-256 файлов. Проверены архитектуры и
динамические зависимости: macOS использует системные dylib/frameworks,
Windows — системные DLL/UCRT; дополнительные MinGW runtime DLL не понадобились.
GUI этих сборок на целевых ОС не запускался; runtime_tested остаётся false.

`scripts/update-binaries.py` собирает и обновляет каталог только после успеха;
`--check` проверяет freshness, архитектуру и payload hashes без компиляции.
Root AGENTS.md требует обновления обеих платформ после изменения build inputs.
`.gitattributes` фиксирует LF, бинарные файлы исключены из text conversion.
Workflow binaries.yml при push исходников собирает macOS ARM64 и Windows x64
на соответствующих runners, проверяет и коммитит оба каталога в ту же ветку.
Есть защита от устаревшего run и рекурсивного rebuild; CI ещё не запускался.
Скачивание с GitHub возможно после публикации локального коммита через push.


## Phase 6 — Terminal Subsystem

- [x] Одиннадцатый workspace crate `nozhik-terminal`; portable-pty 0.9,
  vt100 0.16.2 поверх vte 0.15, parking_lot и Unix-only nix signal API.
- [x] `pty.rs`: native Unix PTY / Windows ConPTY, SHELL/COMSPEC discovery,
  cwd проекта, TERM/COLORTERM, split reader/writer, закрытие slave после spawn.
- [x] `grid.rs`: VT100 parser adapter, 16/256/RGB colors, SGR attributes,
  cursor/hide, wide/combining Unicode, bounded scrollback, normal/alternate grid,
  application cursor и bracketed paste modes, OSC title, DSR/DA responses.
- [x] `session.rs`: background control/start/resize/poll/kill/reap, независимые
  reader/parser и writer, bounded queues/backpressure, immutable Arc snapshots.
- [x] `manager.rs`: уникальные TerminalSessionId, до 16 параллельных сессий,
  create/remove/lookup/events/shutdown, отмена создания при раннем закрытии.
- [x] `terminal_panel.rs`: вкладки/+/close/collapse, cwd tooltip и OSC titles,
  автозакрытие по exit с кодом в status bar; hidden sessions продолжают работать.
- [x] `terminal_view.rs`: cell-aligned monospace painting, ANSI palette,
  фон/стили/блочный курсор, scrollback wheel, geometry → PTY resize,
  focus lock и клавиатурный encoder, UTF-8 и bracketed paste.
- [x] `ui.terminal` в ActionRegistry/Search Everywhere и Alt+F12 в штатных
  JetBrains/VSCode keymaps; Custom сохраняет явные пользовательские bindings.

### Потоки, события и инварианты

`TerminalManager::create(SessionOptions)` выделяет ID и планирует запуск без I/O
на UI. Для каждой сессии control worker владеет MasterPty/Child и опрашивает
try_wait каждые 20 ms; writer пишет пользовательский ввод и device query replies,
reader обрабатывает выходные chunks до 8192 bytes через VT100/vte и материализует
TerminalGrid. Parser защищён parking_lot mutex между reader/control workers.
UI не захватывает parser lock: `snapshot()` использует try_lock только на
готовом SessionSnapshot и клонирует Arc<Grid>. При занятости остаётся предыдущий
снимок. Wake вызывается после освобождения locks, запросив egui repaint.

| API / событие | Контракт |
| --- | --- |
| SessionOptions { cwd, shell, rows, cols } | Native cwd, optional explicit program/args; default shell определяется worker |
| write(Vec<u8>) | До 64 KiB целиком; очередь 64 batches; Busy/Closed/Invalid вместо частичной отправки |
| resize(rows, cols) | 1..=200 rows, 1..=500 cols; очередь control 128; resize PTY и сетки только в worker |
| scroll(i32) | Положительное число — в scrollback, отрицательное — к live screen; alternate игнорирует |
| SessionSnapshot { state, grid: Option<Arc<TerminalGrid>> } | Starting/Running(pid)/Exited(code)/Failed(message)/Closed; grid отсутствует до первого вывода |
| TerminalGrid | rows×cols row-major TerminalCell; UTF-8 cell.text, char, fg/bg, SGR style, wide/continuation, cursor и modes |
| Started { id, pid } | Spawn завершился успешно |
| Exited { id, code } | Child reaped; UI удаляет соответствующую вкладку |
| Error { id, message } | Ошибка spawn/read/write/resize/poll; видна в status/stderr |

Lifecycle events идут через отдельный mpsc, grid — авторитетный snapshot.
UI drain ограничен 128 событиями за кадр. Reader выполняет весь ANSI parse вне UI;
виртуальная сетка охватывает viewport и scrollback до 3000 строк. SGR intensity
в vt100 выбирает bold либо dim; остальные flags комбинируются. Wide/combining
символы сохраняются в cell.text; UI не рисует continuation повторно. Внутренний
wrap-pending cursor vt100 нормализуется к последней видимой колонке snapshot/DSR.

close/Drop выставляют независимый stop flag, поэтому насыщенная input queue не
мешает завершению. Unix shutdown посылает HUP foreground process group, завершает
и reaps shell, затем завершает оставшийся foreground group; Windows использует
Child termination и закрытие ConPTY. Workers получают ограниченное время для
завершения reader/writer; `TerminalManager::shutdown()` ждёт максимум 3 секунды
суммарно при App::on_exit. Обычный frame/закрытие вкладки не ждёт. Умерший shell
не перезапускается автоматически. Явно detached процессы не отслеживаются.

При смене проекта существующие терминалы сохраняют свои cwd; новая вкладка
получает актуальный project root. Без проекта панель показывает приглашение,
процесс не запускается. Скрытие панели не меняет lifetime сессий.

### Проверки Phase 6

Unit-тесты: ANSI 16/256/RGB, SGR flags, cursor/clear/hide, split UTF-8,
wide/combining cells, OSC title, DSR/DA replies; 3000-line scrollback, alternate
screen и restore cursor после resize. Настоящие PTY тестируют echo, kernel winsize,
две сессии и выборочное закрытие, сохранение второй сессии, exit code/reap,
ошибку отсутствующего executable и close при startup. Unix-тест проверяет Ctrl+C
через line discipline и close при насыщенной очереди записи. UI тесты проверяют
control/functional/application keys, UTF-8/paste и Alt+F12 → Action → bottom panel
через headless egui. Отдельный ignored test запускается явно при наличии vi:

```sh
cargo test -p nozhik-terminal system_vi -- --ignored
```

На локальном macOS этот тест прошёл с `/usr/bin/vi`: alternate screen, ввод текста,
выход `:q!`, успешный exit/reap без user config/viminfo/swap files.

### Границы версии

Нет TUI mouse protocol forwarding, mouse selection/copy, history search,
persistent terminal sessions, полной реализации расширений xterm и history reflow.
OSC clipboard не обрабатывается. Геометрия ограничена 200×500, сессий максимум 16;
очереди и scrollback ограничены, но это не OS sandbox. Внешние cargo/composer/htop,
шелл и их конфигурации принадлежат пользователю. GUI Windows/Linux и реальные
PowerShell/cmd/ConPTY здесь вручную не запускались; Windows cross-build проверяется
отдельно, наличие CI matrix не объявляется успешным platform run.


### Верификация Phase 6 (2026-10-26)

На macOS arm64 / Rust 1.99.1 прошли `cargo fmt --all -- --check`,
Clippy workspace/all-targets/all-features с `-D warnings` для native macOS и
cross target `x86_64-pc-windows-gnu`. Полный workspace test run прошёл вне sandbox:
56 unit + 5 integration + 1 doc-test; один system_vi test в обычном прогоне ignored,
затем отдельно запущен с `--ignored` и прошёл. Внутри sandbox прежний FSEvents test
завершается timeout; PTY-тесты прошли и внутри sandbox.

Release macOS ARM64 и Windows x64 GNU пересобраны через update-binaries.py;
обе build.json metadata содержат одинаковый source fingerprint. `--check`
проверяет актуальность и payload SHA-256; Mach-O ARM64 и PE32+ x86-64 проверены.
Реальные GUI Windows/Linux, ConPTY/PowerShell/cmd и GitHub CI в этой итерации
не выполнялись. Нативное окно macOS также вручную не открывалось; проверка vi
использовала настоящую PTY и виртуальную сетку без GUI.


## Phase 7 — Remote Development & SSH

- [x] Двенадцатый крейт `nozhik-remote-ssh`, russh 0.65 (keys встроены),
  russh-sftp 3.0.1, Tokio; без libssh2, ring crypto backend.
- [x] SshProfile/Password/PrivateKeyFile/Agent, default port 22, валидация,
  уникальные имена, атомарный `config/ssh_hosts.json` рядом с settings.toml.
  Секреты запрашиваются модально и не сериализуются; malformed JSON сохраняется.
- [x] Проверка SHA256 host key до авторизации; first-use TOFU dialog с явным
  подтверждением и сохранением в ssh_known_hosts.json, блокировка изменённых
  ключей и host certificates без CA policy. Legacy profile pins поддерживаются.
- [x] Connect/auth/channel deadlines 15s; keepalive 20s/max 3; Unix SSH_AUTH_SOCK
  и Windows OpenSSH pipe, OpenSSH key files с optional passphrase.
- [x] xterm-256color PTY, shell, resize/window-change, Data/ExtendedData/ExitStatus;
  bounded adapter в штатные TerminalView/Emulator/SessionSnapshot и SSH tab labels.
- [x] Tool window ⇄ Remote Host / SSH, профильный modal, Test Connection,
  Connected/Offline, Open Terminal, Browse SFTP, Disconnect, Start/Stop Tunnels.
- [x] SFTP lazy tree, атрибуты, UTF-8 read с лимитом 16 MiB, временный приватный
  локальный кэш; прозрачный upload Ctrl/Cmd+S с проверкой ревизии и сохранением dirty.
- [x] Кэш каталогов: максимум 128 на соединение, TTL 30s; folder-first sorting,
  фильтрация dot entries; явный Refresh и invalidation после upload.
- [x] Local forwarding только 127.0.0.1, port 0, direct-tcpip,
  copy_bidirectional/half-close, максимум 64 потока, cancel listener + streams.

### Архитектура и жизненный цикл

remote_ssh независим от UI/terminal/config. Shared authenticated Handle создаёт
независимые shell/SFTP/tunnel каналы. UI worker владеет runtime, профилями,
сессиями, SFTP, TempDir и туннелями; очереди команд/ответов по 64, wake → repaint.
Сеть/локальный I/O не блокируют UI; общая команда ограничена 35s, отдельные сетевые
операции 15s. При закрытии runtime отменяет задачи; SSH channel adapter использует
отдельные bounded input/output очереди по 64. Терминальный ExternalTransport
не зависит от russh: existing background Emulator обрабатывает ANSI bytes.

Реестр remote documents хранит cache PathBuf → (profile, POSIX remote path).
Remote basename не используется как локальный путь. Повторное открытие сохраняет
несохранённый TextBuffer. Save получает Rope snapshot/revision, upload выполняется
на worker; ack очищает dirty только при той же ревизии. При ошибке/обрыве нет
fallback на локальное сохранение. Смена проекта не меняет remote document identity.

### Проверки и границы

Тесты профилей: roundtrip трёх auth types, default port, пустые/неверные поля,
дубликаты, атомарное сохранение и сохранность malformed JSON. Mock SSH server
на localhost проверяет настоящее рукопожатие/password auth, pinned key rejection,
PTY/resize/echo, direct-tcpip roundtrip и disconnect. Stalled TCP handshake
проверяет штатный 15s timeout без panic. In-memory SFTP protocol server проверяет
read/write/truncation, attributes, directory cache reuse/expiry/invalidation.
Terminal transport test проверяет ANSI grid, input, resize и close. Headless UI
проверяет modal и stale save ack, dirty после обрыва, отсутствие local-save fallback.

Удалённый workspace/LSP/Git, SSH config/ProxyJump, 2FA, автопереподключение,
SSH host certificates/CA и known_hosts import остаются за границами. Форма поддерживает несколько forwards, как и JSON. SFTP upload не атомарен,
внешние изменения/блокировки/merge не проверяются. Лимит editor read 16 MiB;
каталоги читаются целиком, pagination пока нет. Реальные внешние SSH-серверы,
GUI macOS/Windows/Linux и Windows Agent runtime вручную не проверялись.


### Верификация Phase 7 (2026-10-26)

На macOS ARM64 / Rust 1.99.1 прошли fmt, Clippy workspace/all-targets/all-features
с `-D warnings` для native и x86_64-pc-windows-gnu; также прошли
`cargo build --locked --workspace --release --offline` и `git diff --check`.
Полный workspace test run
вне sandbox: 64 unit + 5 integration + 1 doc-test; прежний ручной system_vi test
остаётся ignored и в этой итерации отдельно не запускался. SSH shell cancellation
проверен настоящим локальным сервером при заполненной выходной очереди:
закрытие вкладки доставляет peer channel-close без ожидания освобождения очереди.

Release-бинарники обеих платформ пересобраны scripts/update-binaries.py;
`--check` подтвердил общий source fingerprint, архитектуры и SHA-256 payload.
GUI и внешние SSH-серверы не проверялись; runtime_tested остаётся false.


### Доработка SSH подключения: TOFU и запрос пароля (2026-10-26)

Из формы Add SSH host убрано обязательное поле fingerprint. Новый профиль можно
сохранить без pin. Verify получает ключ сервера без отправки пароля; первое
подключение показывает host/port и fingerprint с Trust and Continue / Cancel.
Доверие сохраняется атомарно в отдельном ssh_known_hosts.json по host/port;
отмена ничего не сохраняет. Изменившийся ключ блокируется отдельным modal
без автоматического replacement. Старые профили с pin остаются совместимыми.
После проверки/принятия ключа открывается Password/Passphrase с автоматическим
фокусом, маскированием и подтверждением Enter; форма поясняет этот порядок.
При настоящем auth handshake ключ снова строго проверяется.

Mock SSH audit проверяет отсутствие password auth до доверия, его сохранение,
повторную проверку, успешную авторизацию после доверия и отказ при смене ключа.
Проверяются malformed trust-file preservation и отсутствие записи при отмене.
Headless egui test вводит пароль в сфокусированное поле и проверяет, что текст
не попадает в отрисованные строки. Профили без поля fingerprint валидны;
непустые legacy pins валидируются парсером OpenSSH fingerprint.


Проверка доработки: fmt, native и Windows GNU Clippy all-targets/all-features
с -D warnings, workspace release build и git diff --check прошли.
Полный прогон: 66 unit + 5 integration + 1 doc-test; system_vi остаётся ignored.
Обе платформы пересобраны update-binaries.py, --check подтвердил source/payload
hashes. Проверки выполнены на mock SSH и headless UI; внешний сервер пользователя
и нативное окно в этой доработке не запускались.


### SSH shell, управление профилями и защищённые пароли (2026-10-26)

- Исправлен ложный server rejected PTY/shell request: ожидание одного сообщения
  заменено ожиданием Success/Failure с пропуском WindowAdjusted/XonXoff. Ранний
  вывод сохраняется в bounded pending queue, SshShellStream::wait возвращает его
  перед дальнейшими сообщениями. Failure различает PTY/shell, close/exit не маскируются.
- Ошибка воспроизведена с прежним waiter против отдельного настоящего OpenSSH
  на localhost; исправленный код открывает PTY/shell, выполняет printf и проверяет
  SFTP write/read. Тест system_openssh_shell_and_sftp ignored по умолчанию,
  запускается явно, использует временные ключи/config без изменения ~/.ssh.
- Edit/Save и Delete с подтверждением, Forget password. Worker перечитывает
  актуальные профили, валидирует уникальность, сохраняет атомарно, отключает
  затронутый транспорт. Ошибки сохраняют форму и старый JSON. Все forwards
  можно редактировать, добавлять и удалять.
- Изменение credential identity удаляет прежний пароль; изменение endpoint/auth/
  имени/legacy pin блокирует upload открытых старых буферов вместо их переназначения
  новому серверу. Текст/dirty сохраняются; cache document IDs не переиспользуются.
- Явный Save password in system credential store в login modal для сохранённых
  профилей. keyring 3.6.3: macOS Keychain, Windows Credential Manager, Linux
  Secret Service с crypto-rust/vendored DBus. save_password default=false —
  единственная metadata в JSON; пароль/фраза сохраняются после успешного auth.
- Native store операции через spawn_blocking, Secret zeroizes собственный буфер
  и скрывает Debug. Пароль загружается после host trust, реальный auth снова
  проверяет host key. Missing/locked/rejected saved password возвращает запрос
  ввода. Ошибки store не создают plaintext fallback и не объявляют запись успешной.
- Удаление/Forget и смена identity удаляют запись OS store; отказ хранилища
  блокирует изменение профиля с сообщением. Trust keys отдельно и не удаляются.

Тесты: настоящий OpenSSH smoke, ранний banner до shell-success, различение
настоящих PTY/shell failures; mock OS credential entry roundtrip/delete/denial
без изменения пользовательского Keychain; worker auth проверяет opt-in save,
повторный вход, rejected/locked/missing credentials и отсутствие секрета в JSON.
Profile tests проверяют edit/rename/delete/duplicate/missing, отказ locked store,
Forget password, сохранение нескольких forwards. Headless UI проверяет
инвалидацию старых документов с сохранением dirty и формы при ошибке записи.


Верификация доработки: fmt, native macOS и Windows GNU Clippy с all-targets/
all-features и -D warnings, workspace release build, git diff --check прошли.
Полный workspace run: 71 unit + 5 integration + 1 doc-test, без failures;
2 ignored (system_vi и system_openssh). system_openssh отдельно запущен и прошёл;
legacy waiter временно воспроизвёл исходную ошибку на том же OpenSSH, затем код
восстановлен. После выделения test setup дополнительно прошли все 6 remote UI/
worker tests. macOS ARM64 и Windows x64 GNU бинарники обновлены скриптом,
--check подтвердил общий source fingerprint и payload hashes. Реальные Keychain/
Credential Manager/Secret Service и GUI вручную не проверялись; OS API тестируются
через изолированные mock/injected stores, Windows API проверен cross-build.


## Phase 8 — Database Tooling

- [x] Тринадцатый crate nozhik-database, SQLx 0.8.6 с Tokio/Rustls и тремя
  драйверами, chrono/uuid/JSON/exact decimal; независим от egui/config/event_bus.
- [x] Валидируемые ConnectionProfile, атомарный database_connections.json рядом
  с settings.toml; malformed preservation, duplicate rejection. Session-only
  Secret; opt-in OS credential store после успешного теста того же профиля/пароля.
- [x] Enum PgPool/MySqlPool/SqlitePool, срок ожидания, owned SSH loopback tunnel
  на свободном порту; существующий TOFU/login flow, отдельный пароль БД.
- [x] DbDriverType, DbValue (Null/Bool/Int/Float/Text/Bytes/DateTime/Json),
  ColumnInfo, QueryResult со временем, affected rows и признаком truncated.
- [x] information_schema PostgreSQL/MySQL/MariaDB; SQLite database_list,
  sqlite_master/table_info; namespaces, Tables/Views, ordered columns, nullability/PK.
- [x] Bounded worker и фоновые query tasks, cancellation_token, SQLite VM progress
  cancellation, закрытие серверного соединения при отмене, disconnect/shutdown.
- [x] Database Explorer справа, New Connection/Test/Save, SSH host selector,
  Online/Offline, Refresh/Disconnect, контекстные Open Table / SQL Console.
- [x] Data Grid с виртуализацией обеих осей, sticky headers, имя/тип,
  Asc/Desc/None, numeric sorting, selection, Ctrl/Cmd+C, CSV/TSV row copy.
- [x] Rope SQL editor tabs, SQL lexical highlighting, connection/schema toolbar,
  Execute и Ctrl/Cmd+Enter, selection/caret statement, отдельные result tabs.

### Контракты, форматы и ограничения

Подробная архитектура — [architecture.md](architecture.md#database-tooling-phase-8).
Tree: DatabaseSchemaTree.schemas → SchemaInfo.tables → TableInfo.columns;
TableInfo.is_view определяет категорию, SchemaColumn содержит nullable/PK.
Все preview identifiers экранируются; catalog имена передаются bind parameters.
Postgres показывает схемы подключённой базы, MySQL — доступные базы,
SQLite — attached namespaces. Для SQLite активную namespace задают квалифицированным
именем в SQL; PG/MySQL selector применяет search_path/USE на pool connection.

Worker queues 32/32, максимум 16 query tasks, retained results 10 000 rows / 16 MiB.
UI коррелирует ответы по request ID, отменяет закрытую вкладку и не меняет её
SQL-буфер при ответе. Data Grid сортирует только полученные строки, не БД.
Нет editable cells, server-side pagination, SQL completion/LSP, persisted console
sessions, отдельных транзакционных SQL-сессий на вкладку, connection edit/delete
или настройки TLS в форме. SQL lexer не полноценный AST процедурных SQL-диалектов.
Транзакции выполняются внутри одного запуска скрипта; отмена/лимит/ошибка не
гарантируют откат уже выполненных операторов и не запускают скрипт повторно.

SQLx выдаёт native types; SQLite JSON остаётся Text (runtime TEXT), SQLite
Boolean/DateTime учитывают declared column type. Exact decimal и unsigned overflow
выдаются Text без перевода в float; Decimal имеет предел точности rust_decimal,
вендорные enum/array/spatial/interval и прочие неподдержанные типы дают явную
ошибку декодирования. Для одиночного пустого SELECT колонки получаются describe;
пустой SELECT внутри multi-statement скрипта может вернуть пустые descriptors.
Лимит значений применяется после получения строки от драйвера. Schema introspection
полная, с запросом колонок каждой таблицы; на очень больших каталогах нет lazy load.
SQL-подсветка лексическая, пересчитывается при изменении ревизии консоли.

### Проверки Phase 8

Тесты проверяют все три serde-профиля, duplicate/invalid/malformed preservation,
SQLite create/insert/typed rows, NULL/Boolean/DateTime/bytes/Unicode, scripts,
empty SELECT columns, PK/not-null/view introspection, file reconnect, syntax error
без потери in-memory данных, cancellation и row limit. Worker тестирует request
correlation и отказ сохранения непроверенного пароля без обращения к OS store.
Headless egui проверяет Explorer/console flow, Execute shortcut без вставки
перевода строки, selection SQL, SQL quotes/comments/dollar strings, numeric sorting,
CSV quoting, bounded viewport paint и focused clipboard copy.

Отдельные ignored system tests запускаются против изолированных временных БД:

```sh
NOZHIK_TEST_PG_PORT=<localhost-port> NOZHIK_TEST_MYSQL_PORT=<localhost-port> \
  cargo test -p nozhik-database system_ -- --ignored
```

Порты принадлежат отдельным тестовым контейнерам, database=nozhik,
Postgres user=nozhik, MySQL/MariaDB user=root, password=nozhik-test-only.
Тесты создают только таблицу phase8 в этой изолированной БД. Проверяются native
JSON/NULL/date/decimal/unsigned types, schema introspection и cancel sleep.
SSH system test поднимает настоящий localhost russh server с временным host key
и передаёт PostgreSQL wire protocol через authenticated direct-tcpip до БД;
целевой host db.internal проверяется сервером, не разрешается локальным DNS.

На этой машине прошли отдельные прогоны с PostgreSQL 12.2, MySQL 8.0.46 и
MariaDB 12.3.2, а также PostgreSQL через SSH. Контейнеры временные, с отдельными
случайными loopback-портами; пользовательские БД не использовались.
Реальные OS credential stores и нативные GUI не проверялись; GUI Windows/Linux
и GitHub CI этой итерацией также не запускались.


Итоговая верификация (2026-10-26): fmt, Clippy workspace/all-targets/all-features
с `-D warnings` для native macOS ARM64 и Windows x64 GNU прошли. Полный workspace
прогон вне sandbox: 81 unit + 5 integration + 1 doc-test, без failures;
4 system tests по умолчанию ignored (два прежних и два database). Оба database
system tests отдельно прошли на PostgreSQL/MySQL, затем MySQL-драйвер дополнительно
проверен на MariaDB. Headless clipboard/viewport test также прошёл.
`cargo build --workspace --release --offline` успешен. Нативное GUI и реальные
OS credential stores не запускались; наличие CI не объявляется его успешным прогоном.

Обе release-платформы обновлены через scripts/update-binaries.py.
`--check` подтвердил общий source fingerprint и SHA-256 payload; проверены Mach-O
ARM64 и PE32+ x86-64. runtime_tested=false сохраняется: нативные окна не запускались.
Тестовые контейнеры и их временные volumes удалены.


## Phase 9 — Системная интеграция / v0.1.0-RC1 (Feature Complete)

- [x] Общий menu catalog + ActionRegistry/KeymapRegistry для File/Edit/View/
  Navigate/Code/Git/Database/Terminal/Tools/Help. macOS native menu/accelerators
  через muda; embedded MenuBar на Windows/Linux и при native initialization failure.
- [x] New/Open/Save/Save All/Close/Settings/Exit, Undo/Redo/Cut/Copy/Paste,
  Find/Replace/Replace All, MRU 30 путей, Git Fetch/Pull/Push/Branches,
  Database connection/console, New Terminal и About с build identity.
- [x] Семантические theme tokens, ANSI base palette, радиусы, resize handles,
  status/icon tooltips. Drag docks и SQL editor/results, атомарный layout.json.
- [x] Несохранённые file/SQL tabs: Save All/Discard/Cancel для Close Tab и окна.
  Gate ждёт saved revision, сохраняет dirty при ошибке/гонке и отменяет выход.
  Закрытые вкладки освобождают Rope/history/Data Grid; paths остаются в MRU.
- [x] Idempotent shutdown LSP→terminal→SQL pools→SSH tunnels/sessions→plugins;
  bounded acknowledgements, Pool::close, app event consumer/runtime teardown.
- [x] Event-driven repaint bus/watch/workers вместо 100ms redraw. Исправлен
  просроченный timer скрытой Git-панели без repo; unchanged plugin snapshots
  не будят UI каждую секунду.
- [x] tests/stress_tests.rs подключён как integration target nozhik-ui:
  10 MB Rope edits + 100k-line viewport, 100k×50 Data Grid + scrolling,
  streaming real PTY + Git polling + active mock LSP buffer.
- [x] Lifecycle/save failures/stale revisions/menu registration/idle repaint/
  splitter dragging + layout persistence проверяются headless egui тестами.

### Проверка производительности и границы RC1

Локальный debug stress run: 10 MB Rope edits + viewport ~16 ms;
синтетическая таблица 5M ячеек + 10 layout passes ~97 ms, максимум 197 видимых
text shapes в окне 1200×700. Это диагностические стендовые результаты с широкими
регрессионными timeout, не FPS benchmark и не абсолютная гарантия задержек.
Backend SQL limits (10k rows/16MiB) в реальном console остаются; stress grid
намеренно конструирует большой snapshot напрямую, проверяя UI независимо от них.

Нативный release macOS ARM64 запущен с отдельными temporary settings и пустым
non-Git проектом. AX подтвердил верхнее меню и availability File items.
Дальнейшая визуальная/мышиная QA остановлена блокировкой экрана macOS.
Профиль idle после исправления hidden Git timer: 0.18 CPU s за 30.01 wall s,
**0.6% одного ядра**, screen locked, без активных LSP/PTY/DB запросов.
Показатель зависит от ОС/дисплея/драйверов и фоновой нагрузки; unlocked foreground
и Windows/Linux измеряются по release checklist, универсальное CPU≤1% не заявляется.

RC1 — полнота текущего заявленного набора функций, не отмена прежних ограничений:
runner/debugger, marketplace, full accessibility, signed installers, SQL editing/
pagination, многие vendor SQL types и persistent sessions остаются вне scope.
Manual release matrix и неисполненные gates отмечены в verification-checklist.md.


### Итоговая автоматизированная верификация Phase 9 (2026-10-26)

На macOS ARM64 / Rust 1.99.1 прошли fmt, Clippy workspace/all-targets/all-features
с `-D warnings` для native и cross x86_64-pc-windows-gnu. Полный workspace run:
87 unit + 8 integration (включая 3 stress) + 1 doc-test = 96 passed, без failures;
4 прежних system tests остаются ignored. Release workspace build успешен.
Headless pointer test подтвердил настоящее перетаскивание splitter и сохранённую
пропорцию; тест clean close подтвердил освобождение большого буфера.
Тестовый нативный процесс после измерения CPU завершён, временный bundle удалён.

Финальные macOS ARM64 и Windows x64 GNU binaries пересобраны через
scripts/update-binaries.py. `--check` подтвердил одинаковый source fingerprint
и SHA-256 обоих payload. runtime_tested=false сохранён: runtime-матрица новых
скачиваемых комплектов целиком не выполнена. Нативное меню/idle smoke проверялись
на локальном промежуточном release build; оставшиеся GUI gates явно открыты.


### Системный выбор папок, Recent Projects и New Project (2026-10-26)

- Project: сохранён ввод пути/Open, добавлена Choose folder… с окном ОС; под
  кнопками — прокручиваемая история успешных открытий, свежие сверху, клик открывает.
- File: Open File / Folder… сразу открывает native picker. AppKit на macOS
  поддерживает одновременно файлы/папки; Windows/Linux используют folder mode
  и соседний Open File… для выбора файла. File → Recent Projects — hover submenu,
  общий источник данных с панелью Project, включая native macOS menu.
- File → New…: OS parent picker → имя → exclusive create_dir → открытие проекта.
  Валидация одного компонента имени, Unicode/пробелы, запрет traversal/Windows
  reserved names; existing directory не перезаписывается, ошибка сохраняет форму.
- Отдельные fs_model replies ProjectOpened/ProjectCreated/ProjectCreationError:
  раскрытие дерева не обновляет MRU, ошибки создания не смешиваются с Save gate.
- recent_projects.json рядом с settings: canonical paths, Unix milliseconds,
  дедупликация/сортировка, ordered background I/O, atomic tempfile replacement,
  malformed-file preservation, wake callback и bounded shutdown flush.
  Уже открытые документы и dirty text сохраняются при переключении проекта.

Проверки: fmt, native macOS и Windows GNU Clippy workspace/all-targets/all-features
с -D warnings, workspace release build, git diff --check прошли. Полный прогон
вне sandbox: 91 unit + 8 integration + 1 doc-test = 100 passed, 4 прежних system
теста ignored. В sandbox watcher test упёрся в timeout; вне sandbox прошёл.
После финальных изменений отдельно повторены 31 UI unit + 3 stress tests.

Native macOS release smoke с отдельными temporary settings и non-Git папками:
проверены File menu, настоящее NSOpenPanel, выбор/открытие папки, порядок истории,
открытие из Recent Projects, форма New с выбором parent и вводом имени через UI,
создание/открытие новой папки и её добавление в MRU. Визуально проверена панель
с кнопкой и историей; тестовое приложение завершено через File → Exit.
Windows/Linux GUI этой итерацией не запускались. Обе платформы пересобраны
scripts/update-binaries.py; --check подтвердил общий source fingerprint и SHA-256
payload. runtime_tested=false оставлен для неполной межплатформенной runtime matrix.


### Иконки и семантические цвета Project (2026-10-26)

- Векторные папки и chevrons раскрытия, компактные значки основных языков,
  конфигурации, Markdown, Docker, SQL, изображений и архивов. Для неизвестного
  типа — серый документ; внешние шрифты/emoji и зависимости не добавлены.
- Ignored/service paths видны красным на приглушённом красном фоне, раскрываются
  лениво; `.git` скрыта. Индекс поиска сохраняет прежние исключения. Snapshot
  хранит is_excluded, учитывая вложенные ignore/negation rules. Watcher обновляет
  и отображаемые ignored entries, кроме `.git` internals.
- Тестовые папки/файлы, их потомки и настройки линтеров зелёные. Красное
  исключение имеет приоритет; выбор/hover/focus остаются видимыми.
- Project path/Open/Choose folder/Recent Projects показываются только без
  открытого проекта; верхнее File menu сохраняет открытие и историю.

Проверки: fmt, native и Windows GNU Clippy workspace/all-targets/all-features
с -D warnings, workspace release build, git diff --check прошли. Полный прогон
вне sandbox: 93 unit + 8 integration + 1 doc-test = 102 passed, 4 прежних system
tests ignored. В sandbox OS watcher test дал timeout; вне sandbox прошёл.
Тесты покрывают видимость ignored paths, потомков vendor, nested ignore/negation,
снятие ignore при обновлении, классификацию иконок и скрытие стартовых действий.
Нативный macOS release визуально проверен на отдельном временном проекте и
settings: папки/стрелки, красные vendor/node_modules/.env, зелёные tests/phpstan,
типовые значки и серый неизвестный файл, отсутствие открытия/истории над деревом.
Тестовое окно закрыто. Windows/Linux GUI не запускались.
Обе платформы пересобраны scripts/update-binaries.py; --check подтвердил общий
source fingerprint и SHA-256 payload. runtime_tested=false сохранён для неполной
межплатформенной runtime matrix.


### Оформление по референсу и локальные бинарники (2026-10-26)

- `binaries/` удалён из индекса Git с сохранением локальных файлов и добавлен
  в .gitignore; CI больше не создаёт коммиты бинарников, сохраняет artifacts.
  README и AGENTS.md закрепляют локальную сборку обеих платформ после доработок.
- Единые скруглённые поверхности Project/Git/Remote, Database, редактора и
  терминала, промежутки между ними, компактные rails/toolbar и строки дерева.
  Палитра адаптирована к референсу; новых функций нет.
- Векторные иконки Project, Git, Structure, Remote, Database, Notifications,
  Settings, Search, Run/Debug и Git menu вместо отсутствующих Unicode-глифов.

Проверки: fmt, workspace Clippy all-targets/all-features с -D warnings,
workspace release build и git diff --check прошли. Полный workspace test run
вне sandbox: 93 unit + 8 integration + 1 doc-test = 102 passed, 4 прежних system
теста ignored. В sandbox FSEvents/localhost SSH ограничены; полный прогон вне
sandbox успешен. Обе платформы пересобраны update-binaries.py; --check подтвердил
общий source fingerprint и SHA-256 payload локальных сборок.

Нативная визуальная проверка macOS release с временными проектом/settings:
Project, раскрытие папки и открытие PHP-файла, редактор, Database и Terminal,
скругления/отступы и иконки боковых rails и toolbar. После финального исправления
Git-иконки приложение перезапущено и отсутствие квадратов подтверждено.
Тестовое приложение закрыто. Windows/Linux GUI не запускались;
runtime_tested=false сохранён для неполной runtime matrix.

### Code Styles, форматирование и Settings (2026-10-27)

- config::code_style: строгий JSON CodeStyle, встроенный php_psr12.json,
  каталог по расширениям, background scan home/config directories, явный None
  и отсутствие профиля без принудительного форматирования. Выбор профиля и
  syntax palette сериализуется в settings.toml/JSON.
- code.reformat в общем menu/action catalog и Ctrl/Cmd+Alt+L в keymap defaults.
  Форматируется selection или весь буфер. Приоритет LSP formatting/rangeFormatting,
  fallback PHP Tree-sitter whitespace formatter по правилам JSON, проверка AST
  после форматирования, одна транзакция Undo. Stale responses проверяются по
  token, BufferId, revision и текущему профилю. Mixed HTML/heredoc/malformed PHP
  локальный formatter не поддерживает; произвольные vendor LSP rules зависят
  от настроек сервера, через protocol передаются tabSize/insertSpaces.
- Smart Paste нормализует внешний отступ и сохраняет относительную вложенность,
  отдельный контекст каждого курсора и единая транзакция. None оставляет clipboard
  без изменений. Однострочная вставка остаётся дословной.
- AST ERROR/MISSING и unmatched {}, (), [] без строк/комментариев; маркеры
  opening/expected closing, красные волны и tooltip. Parser diagnostics отделены
  от LSP и не исчезают при получении server diagnostics.
- settings_dialog.rs: General / File Types & Code Styles, font family/size,
  текущая New UI Dark theme, выбор JSON-профиля и syntax palette, Reload profiles,
  фоновые последовательные atomic saves и bounded shutdown flush.

Верификация: 111 passed, 0 failed, 4 прежних system tests ignored в полном
workspace/all-features прогоне вне sandbox. В sandbox OS watcher упёрся в timeout;
вне sandbox прошёл. Новые тесты проверяют PSR-12 output и идемпотентность,
selection boundaries, literals/comments/for headers, unclosed delimiters,
Smart Paste/None, single Undo, disk profile selection/persistence и настоящие
formatting/rangeFormatting requests к mock stdio LSP. fmt, native macOS и Windows
GNU Clippy workspace/all-targets/all-features с -D warnings прошли.
Нативное GUI и Windows/Linux runtime этой итерацией не запускались.

Финальные macOS ARM64 и Windows x64 GNU binaries обновлены через
scripts/update-binaries.py; --check подтвердил общий source fingerprint и
контрольные суммы обоих комплектов. После выделения General UI в helper отдельно
повторены 34 UI unit + 3 stress tests вне sandbox: все прошли. При попытке этого
повтора в sandbox два localhost SSH теста получили PermissionDenied; вне sandbox
ошибка не воспроизводится. runtime_tested=false: межплатформенная GUI-матрица
не запускалась.

### Исправление размеров Settings (2026-10-27)

Устранено разрастание модального окна по высоте из-за вертикального Separator
в unbounded Area. Размеры ограничены viewport, обе панели имеют конечную высоту,
содержимое настроек прокручивается с явно вертикальной раскладкой. На узких
экранах категории отображаются сверху; заголовок и Close не прокручиваются.
Регрессионный headless egui тест воспроизвёл прежнее превышение высоты, затем
проверил обе категории в 1440×900, 800×600 и 420×280: границы модального окна,
видимость полей и настоящие pointer press/release на Close.

Проверки исправления: полный workspace/all-features прогон вне sandbox —
112 passed, 0 failed, 4 прежних ignored. fmt, native и Windows GNU Clippy
workspace/all-targets/all-features с -D warnings и git diff --check прошли.
Обе release-платформы пересобраны update-binaries.py; --check подтвердил исходники
и SHA-256 обоих комплектов. На отдельной нативной macOS release-копии с временными
проектом/settings визуально проверены General, File Types & Code Styles и Close;
тестовый экземпляр завершён через Cmd+Q. Windows/Linux GUI не запускались.

### Editor mouse input / Reformat Code / Smart Paste fixes (2026-10-27)

- LMB places the caret; drag selects across rows without panning; Shift-click,
  Unicode identifier double-click, whole-line triple-click, wheel on both axes.
- Empty and no-op LSP formatting responses now fall back to Tree-sitter.
  Tagless PHP snippets are supported; selected method calls regain AST indentation
  while neighboring lines remain unchanged. The action initializes/synchronizes
  language state and applies one undoable transaction to the active buffer.
- Single-line and malformed multiline paste normalize target indentation; replacing
  the entire leading prefix prevents doubled indentation. The PHP fragment lexer
  repairs structural nesting while preserving quoted delimiters. Multiline literals
  and heredocs remain verbatim; None retains the original clipboard and range.
- Regressions cover the exact Sample/test/doSomething output (4/8 spaces), inserting
  $user->save() at columns 0/3/8 of an 8-space prefix, crooked nested blocks,
  Event::Paste and Undo, pointer click/drag/word/line/Shift/wheel, real action routing,
  empty-LSP fallback and nonempty-LSP priority with a mock stdio server.

Full workspace/all-features tests outside sandbox: 118 passed, 0 failed, 4 existing
system tests ignored. Native and Windows GNU Clippy all-targets/all-features with
-D warnings, fmt and git diff --check passed. Workspace release build succeeded.

Both local release platforms were rebuilt with scripts/update-binaries.py; --check
verified the common source fingerprint and both payload checksums. Native macOS
release smoke on an isolated temporary project/settings confirmed Cmd+Alt+L on
Sample.php, drag selection without panning, clipboard paste of $user->save() into
an empty method line with 8-space indentation, and Undo back to the original clean
buffer. The test app was closed via Cmd+Q; temporary files were removed.
Windows/Linux GUI were not run; runtime_tested=false remains for the full matrix.

### Enter indentation / typed print inside if (2026-10-27)

The exact check($condition, $message) example is covered: AST formatting restores
8 spaces before print('test'). Enter now inserts the newline and profile-based
indentation atomically, preserving LF/CRLF and ignoring quoted/commented brackets.
Without a profile, Enter stays literal. A regression types the example character
by character into a dirty editor buffer, invokes code.reformat via ActionRegistry,
waits for the worker revision, and checks that one Undo restores the typed source.
Nonempty/partial LSP results additionally receive local structural indentation
repairs, while server token/spacing changes and selection boundaries are preserved.

Verification: full workspace/all-features run outside sandbox — 122 passed,
0 failed, 4 existing system tests ignored. Native/Windows GNU Clippy with all
features/targets and -D warnings, fmt, git diff --check and workspace release build
passed. The initial sandbox-only UI run hit localhost SSH PermissionDenied; the
full run outside sandbox passed. Both local binaries were rebuilt via
update-binaries.py and --check verified the common source fingerprint and payload
checksums. Native GUI was not rerun for this change; keyboard behavior is covered
by real egui key/text events and the asynchronous action/worker regression.

### Formatting PHP around heredoc/nowdoc and markup (2026-10-27)

Removed the file-wide markup/heredoc rejection. Literal bodies, delimiters and
flexible closing indentation remain byte-for-byte unchanged while surrounding PHP
is formatted. Mixed HTML/PHP preserves markup and tag boundaries. A protected-text
comparison also rejects LSP payload rewrites and selects the local fallback.
Regression fixtures include the exact supplied php82-smoke.php (data only, never
executed), its INI payload, print indentation, idempotence, nowdoc, CRLF, selection
inside a payload, malformed heredoc rejection and mixed HTML/PHP.

Verification: 127 passed, 0 failed, 4 existing system tests ignored in the full
workspace/all-features run outside sandbox. Native/Windows GNU Clippy with all
features/targets and -D warnings, fmt, git diff --check and workspace release build
passed. macOS ARM64 and Windows x64 binaries were rebuilt with update-binaries.py;
--check verified common source fingerprint and payload checksums. The supplied
file is unchanged and matches the regression fixture byte-for-byte. Native GUI
was not rerun for this parser/formatting change; no PHP test script was executed.

### Offline PHP standard library and Darcula completion (2026-10-27)

- Added schema v1 in language_support::php_stubs: functions/methods, typed
  parameters (optional/variadic/reference/default), classes/interfaces with public
  members, constants, keywords, docs and insertion snippets with `${0}`.
  Embedded PHP 8.5 starter preset contains 70 top-level entities, including every
  entity requested for this iteration; class members are additional.
- Global persisted PHP Language Level in Settings → File Types & Code Styles →
  PHP/PHTML, 7.0–7.4 / 8.0–8.5. Serial worker loads the requested version from
  ~/.nozhik/languages/php/versions; NotFound alone uses builtin 8.5. Invalid files
  retain the old catalog and report an error. UI displays the actual loaded level.
- Completion works immediately without PHP/LSP, from one identifier character or
  Ctrl+Space. The bounded 500 px / 9-row scrollable popup has rounded Darcula
  surfaces, colored round fx/C/I/c/k icons, accented matching prefixes, elided
  signatures and right-aligned return types. Up/Down scroll the selected row;
  hover/LMB select, double click/Enter/Tab apply, Esc/outside clicks dismiss.
  Functions insert parentheses and put the caret inside; constants/types/keywords
  remain plain text. Leading namespace separators and Class::member prefixes
  are preserved. LSP completions merge with offline results without equal labels.
- Reflection CLI supports PHP 7.0+ and Docker image workflows, all internal
  functions/classes/interfaces and loaded extension constants, tentative return
  types, public members and sibling-temp-file output. Added a Rust example to
  validate generated catalogs before installation. No mass-generated catalogs or
  Docker startup logic are bundled into the IDE.

Verification: full workspace/all-features tests outside sandbox: 133 passed,
0 failed, 4 existing system tests ignored. The sandbox repeat hit the pre-existing
FSEvents timeout; the unrestricted full run passed. Native and Windows GNU Clippy
workspace/all-targets/all-features with -D warnings passed. Headless regressions
cover schema roundtrip, version fallback/error/mismatch, required prefix matches,
real key/text input, Up/Down wrap, insertion/caret/Undo, Ctrl+Space/Esc, real pointer
selection/double click/dismissal, constants/FQNs/members, version ComboBox clicks,
settings persistence/default/validation and small-screen Settings bounds.

Actual Docker runs generated 3070 entities for PHP 7.4, 3148 for PHP 8.1 and 3201
for PHP 8.2; all three full files passed the Rust parser. Pulling php:8.5-cli-alpine
failed with Docker `no space left on device`; PHP 8.5 generator runtime and PHP
7.0–7.3 runtime were not tested. Existing images/data were not pruned.

Both macOS ARM64 and Windows x64 GNU release binaries were rebuilt with
update-binaries.py; --check verified the common source fingerprint and both
payload checksums. Native macOS release smoke used a temporary project/settings:
opened sample.php, showed Ctrl+Space completion with no LSP, checked popup styling,
accepted print_r with Enter, inserted a character inside parentheses and saved;
opened PHP settings, selected 7.4, verified persisted settings and actual fallback
8.5 label. Test app exited through Cmd+Q. Windows/Linux GUI were not run;
runtime_tested=false remains for the full platform matrix.

Final verification also passed `cargo build --workspace --release`, fmt and
`git diff --check` after the implementation and documentation updates.

### Native-only local builds and Windows cache cleanup (2026-10-27)

At the user's request, routine local builds now refresh only the host platform.
AGENTS.md supersedes the former requirement to rebuild both platforms after every
change. Windows cross-build/Clippy on this Mac requires an explicit user request.
`update-binaries.py` keeps native build defaults and now defaults `--check` to the
host platform too. Explicit `--target`, `--check --platform windows-x64` and
`--check --all-platforms` retain on-demand Windows/all-platform support.
GitHub Actions still checks/builds Windows on remote runners.

Removed only target/x86_64-pc-windows-gnu (about 3.4 GiB allocated); native debug/
release caches, installed cross-toolchain and packaged Windows executable remain.
A dry run showed `cargo clean --target` would remove native artifacts as well,
so cleanup directly removed the named Windows directory. README documents that
scoped cleanup. target is now approximately 25 GiB, chiefly 22 GiB native debug.

Six mocked CLI routing checks passed for native build/check, explicit Windows
build/check, all-platform checks and native Windows checks. Native release was
rebuilt with the default script invocation; only macOS metadata was refreshed.
The Windows package retains its previous build metadata and may be stale until
an explicit Windows rebuild. Native fmt/Clippy passed.

Final native verification: workspace/all-features tests — 133 passed, 0 failed,
4 existing ignored; workspace release build, fmt, Clippy and git diff --check
passed. Default --check verifies macOS successfully. No Windows target directory
was recreated by these native commands.

### Full generated catalogs for every PHP level (2026-10-27)

Replaced the 70-entity starter preset with generated JSON for all 11 supported
levels: PHP 7.0–7.4 and 8.0–8.5. Each inventory was generated by the PHP Reflection
script inside the corresponding official php:<level>-cli-alpine image, Linux/amd64.
Exact patch versions and counts are listed in crates/language_support/data/README.md;
catalog-manifest.json stores image digests, loaded extensions and SHA-256 hashes
for each JSON and the PHP generator. All 11 generator hashes agree. Total JSON
size: 30.66 MiB; summed across versions: 34174 global entities, 29313 methods,
4532 class/interface constants (not counts of distinct cross-version symbols).

Generation was strictly sequential. Each container used --rm, and each newly
pulled image/tag and its unused layers was removed before the next version.
Final Docker checks found no nozhik-php-catalog containers or php:*-cli-alpine
images; original counts remained 31 images / 30 containers / 32 volumes / 624
build cache entries. No global prune or cleanup of unrelated data was used.
The reproducible scripts/generate-php-catalogs.py utility preserves pre-existing
images, validates output before replacement, records provenance per version and
supports --version/--docker/--keep-images. The PHP generator additionally emits
missing language constructs and context-dependent magic constants.

All version files are embedded in the application. Missing per-user files now
select the bundled catalog of the requested level instead of PHP 8.5. Existing
user catalogs retain priority; parse/version errors retain the previous catalog.
All 11 files were also installed without overwriting differing existing files in
~/.nozhik/languages/php/versions/ and validated with the Rust example. Catalogs
cover all internal symbols of the images' loaded extensions (32–36 extensions),
not arbitrary optional extensions such as intl/gd/mysqli. Legacy Reflection may
lack types/docs; such fields remain mixed/empty rather than invented metadata.
Platform-dependent constant values describe the Linux/amd64 generating runtime.

Full inventories exposed prefix ranking and initial-loading issues: lowercase
str now prioritizes case-matching names before STREAM_* constants exhaust the
100-result limit, preserving strlen and str_contains. Keyboard/mouse completion
regressions use actual larger result sets. Full JSON parsing is worker-only,
lazily triggered by an open PHP document or Settings; empty workspaces do not
load PHP catalogs. This preserves the idle repaint regression, which initially
failed when a full catalog was eagerly loaded in an empty workspace.

Verification covers every bundled level, required functions/classes/constants,
version boundaries for str_contains (8.0), array_is_list (8.1), json_validate (8.3),
array_find (8.4), array_first (8.5), fn/match/readonly and UnitEnum; user-file
priority, missing files, invalid JSON/mismatched versions, insertion/caret/Undo,
keyboard and pointer behavior. Native fmt/Clippy and full test results are below.

Final verification: 134 passed, 0 failed, 4 existing ignored in the full native
workspace/all-features run outside sandbox. fmt, native Clippy all-targets/
all-features with -D warnings, workspace release build and git diff --check
passed. All 11 installed files passed the Rust parser and match their project
files and manifest checksums. Native macOS binaries were rebuilt via the default
update-binaries.py command and --check verified their source/payload hashes.
No Windows build or Windows cache was created. GUI was not rerun for this data/
loading iteration; completion input, version selection and idle behavior are
covered by headless egui regressions. runtime_tested=false is retained.

### Local AI Inline Completion / Ollama (2026-10-27)

Added nozhik-ai-completion and UI integration for raw Qwen FIM. Context is
Unicode-bounded (2000 prefix / 1000 suffix), generation has a 1500 ms deadline,
required stop tokens and a 128-token output budget. The app-owned Tokio runtime
runs debounce (default 350 ms), HTTP and model downloads. Owned task cancellation
covers new input, navigation, clicks, focus loss, document/settings changes and
shutdown. Revision/caret/BufferId gates reject stale results; offline generation
errors remain silent. There is no periodic AI repaint or automatic download.

Ghost text uses muted translucent #7A7E85 and a Tab hint. Virtual rows move the
real suffix/following lines without editing the Rope. Tab inserts once with one
Undo; Esc/input/navigation dismiss. Existing PHP/LSP popup remains available,
with ghost acceptance taking precedence on Tab. Multiple cursors/selections and
SQL tool consoles do not request AI in this iteration.

Settings → AI Assistant (Ollama) persists enable/model/debounce, inspects tags,
shows Offline/Connected/Installed, links to Ollama, and explicitly downloads the
selected model with current-layer byte/percentage progress. NDJSON is decoded
across arbitrary HTTP chunks; errors/incomplete EOF cannot mark a model ready.
The model field is locked during pull, completion refreshes installed tags, and
Refresh service status allows retry after starting Ollama. No model was downloaded
and no existing Ollama installation/settings were changed during development.

Verification: native workspace/all-features tests passed (141 passed, 0 failed,
4 existing system tests ignored). New tests cover Unicode FIM boundaries/tokens,
raw HTTP payload/stop tokens, debounce replacement, abort closing the old HTTP
connection, fragmented pull records, server errors/incomplete streams, real egui
Tab acceptance/caret/one Undo/Esc/arrows/typing, and multiline/suffix/indent layout.
Existing idle repaint and PHP keyboard/pointer completion regressions also pass.
Ollama protocol tests use local mock HTTP servers; live model quality/download
and native GUI appearance were not manually tested. Windows/Linux were not run.

Final native checks: fmt, workspace/all-targets/all-features Clippy with
-D warnings, workspace release build and git diff --check passed. The final HTTP
fixtures were rerun successfully (5 tests). scripts/update-binaries.py rebuilt
macOS ARM64 and its app archive; default --check verified source and payload
hashes. No Windows build or Windows cache was created.

### Ollama lifecycle follows Nozhik (2026-10-27)

Enabled AI now starts the local Ollama service when absent and preloads the
already-installed selected model during application startup. The application
owns the lifecycle task, coalesces persisted enable/model updates, and awaits
cleanup before stopping Tokio. Turning AI off unloads the model and stops an
owned service; switching models unloads the previous selection. Normal close
cancels editor/download tasks, sends keep_alive=0 to release the selected model,
and terminates/reaps the owned service process tree. A pre-existing external
service is preserved. No model files are deleted or downloaded implicitly.

Ollama CLI discovery supports PATH and standard macOS/Windows installation paths;
owned service host is pinned to loopback. Startup/preload are cancellable, unload
is bounded to five seconds. Unix uses a private process group, Windows a bounded
PID-scoped taskkill /T plus child reaping. Selected-model intent is recorded even
when initially offline, so later manual service startup/download/generation still
receives cleanup on exit. Standard five-minute idle residency remains as a
fallback for external services after abnormal application termination.

Verification: full native workspace/all-features tests — 144 passed, 0 failed,
4 existing ignored; fmt, workspace Clippy all-targets/all-features with -D warnings,
workspace release build and git diff --check passed. Final AI tests rerun: 8 passed.
New tests use local mock HTTP and real isolated Unix child processes to verify
preload/unload request ordering, previous-model release, preservation of an
external service, disabled startup, child termination/reaping and cancellation
of startup during close. Real Ollama/model residency and native GUI were not
manually exercised; Windows process-tree termination was not run on this Mac.

macOS ARM64 binaries/app archive were rebuilt with update-binaries.py; default
--check verified source fingerprints and payload hashes. No Windows build/cache
was created.


### Real Ollama verification and idle-caret completion (2026-10-27)

The installed Ollama service returned an empty model inventory; the selected
qwen2.5-coder:1.5b-base had never been downloaded. Downloaded the official model
(986,060,385 bytes on disk, digest 02e0f2817a89…) and verified actual raw FIM:
PHP add($a,$b) returned `return $a + $b;`. Measured cold request about 1.02 s
(including 0.78 s model load), warm request about 0.05 s on this Mac.

Fixed the edited-revision-only gate: an opened/refocused document and a stopped
caret now schedule completion without typing. File opening/tab activation request
editor focus. One request is issued for each settled position; Esc suppresses
that position and successful/empty results do not create an idle request loop.
Transient errors get two bounded retries. GenerationError preserves meaningful
missing-model/offline/timeout feedback in the editor status bar instead of
silently dropping it. Generation allows 60 s for cold loading and 64 predicted
tokens at temperature 0.2; the one-second connect deadline and cancellation remain.

Fixed native mouse hit testing for click/release events: a completed click is
handled even if the press event was in an earlier frame, and fast same-frame
press+release uses interact_pointer_pos after press_origin has been cleared.
Existing drag/word/line/Shift-selection behavior is retained.

Verification: native full workspace/all-features — 146 passed, 0 failed,
5 ignored (four existing system tests plus the new explicit live-Ollama test).
The live-Ollama test was run explicitly and passed: no text events, real model
result, ghost painting, Tab insertion and one-step Undo. New regression covers
initial idle generation, caret-only changes, a 1600 ms mock response and Esc;
a separate fast-click regression verifies an empty indented line. Existing mouse
and popup regressions passed. fmt, Clippy workspace/all-targets/all-features with
-D warnings, release build and git diff --check passed.

Native macOS smoke used isolated temporary settings/project and actual Ollama:
opened sample.php, placed/moved the caret inside a blank function body without
printing characters, observed ghost text, accepted/saved real generated code.
After closing the test app, /api/ps returned an empty list and the pre-existing
external Ollama service remained alive. The temporary debug-only input trace was
removed. Model files remain installed for the user's IDE. macOS binaries were
rebuilt by update-binaries.py and source/payload checks passed. No Windows build
or Windows cache was created; Windows/Linux GUI were not exercised.

### Standard-library hover documentation and browser navigation (2026-10-27)

Added language_support::doc_provider with LanguageDocProvider, shared EntityKind,
SymbolDocInfo and DefinitionTarget (external URL / future local file coordinates).
The active PHP version catalog implements exact globals/FQN/Class::member lookup,
case-insensitive callables/types and case-sensitive constants. A generic
DocumentationSource binds the provider to editor views without PHP URL logic.

Bundled php_doc_rules.json selects official short php.net aliases. Optional
~/.nozhik/languages/php/doc_rules.json replaces it during worker catalog loading;
category templates support lowercase/underscore transforms and a fallback alias.
HTTPS/name placeholders are validated; invalid files retain the old UI catalog
and remain untouched. Fallback selects a missing category rule, not a network
retry. Existing Reflection descriptions may be empty and are not fabricated.

Cmd (macOS) / Ctrl (Windows/Linux) hover paints a token underline and hand cursor,
and an immediately shown bounded Darcula documentation popup with signature,
existing prose and platform/destination hint. JetBrains Mono Regular is embedded
for this popup under SIL OFL 1.1. Hit testing uses visible Galley glyph coordinates
and UTF-8 boundaries, excludes gutter/punctuation/variables and known highlighted
string/comment spans. Press/release on an inspected symbol preserves CursorSet;
the shell opens the provider URL on a separate named browser thread. Existing
LSP hover and project definition requests remain available. Project shadowing,
import aliases and instance type inference require LSP/a future project index.

Verification: full native workspace/all-features tests passed outside sandbox:
150 passed, 0 failed, 5 existing ignored. New tests cover var_dump/DateTime/
PHP_VERSION URLs, methods/class constants/interfaces, transformations/fallback,
active-version boundaries, case rules, unknown symbols and invalid rule-file
preservation; real headless egui frames cover popup/hand, modifier release,
press/release target, unchanged caret/text and ordinary unknown-name clicks.
Geometry tests cover Unicode/FQN/static members, variables, punctuation, gutter,
trailing area and platform modifier selection. Native fmt, workspace Clippy
all-targets/all-features with -D warnings, release workspace build and
 git diff --check passed. The sandbox run stopped at existing local TCP tests
with PermissionDenied; the full unrestricted run passed. Browser launch and
native GUI appearance were not manually exercised; headless tests inspect the
navigation target without launching a browser. Windows/Linux were not run.

macOS ARM64 executable and Nozhik.app.zip were rebuilt by the default
scripts/update-binaries.py command; default --check verified source fingerprints
and payload hashes. No Windows build or Windows target cache was created.

### Whitespace-only line deletion (2026-10-27)

Unmodified Backspace and Delete now share smart blank-line behavior: excess
indentation before the caret is removed to the expected visual column; at or
before that column, the whole line is removed. Expected indentation uses the
preceding nonblank line and the existing Enter CodeStyle rule; without a profile,
it uses that line's leading whitespace. Tab stops use profile size/fallback tab
width, and a partial tab becomes spaces when needed to reach the exact column.
Line removal consumes the preceding LF/CRLF and lands at the previous line end;
first-line deletion consumes its following newline. Selections, nonblank text
and modified keys retain ordinary deletion. Overlapping multi-caret deletions
are coalesced into one transaction and one Undo step per key.

Headless egui key events verify both keys with 12→8-column snapping, subsequent
line removal, exact caret positions and two-step Undo restoring the original
clean buffer. Additional cases cover tabs/mixed whitespace, off-grid targets,
CRLF, below-target carets, first/last/empty lines, selection preservation, Unicode
context and overlapping/adjacent multi-caret line deletions. No document parser,
full-buffer String or filesystem I/O was added to this input path.

Native full workspace/all-features tests outside sandbox: 152 passed, 0 failed,
5 existing ignored. fmt, native workspace Clippy all-targets/all-features with
-D warnings and git diff --check passed. Native GUI was not manually exercised;
Windows/Linux were not run.

Workspace release build passed. The default update-binaries.py rebuilt macOS
ARM64 executable/app archive and --check verified source/payload hashes.
No Windows build or Windows target cache was created.

### Differential project symbol index and PHP semantic UI (2026-10-27)

Added language_support triggers.rs, symbol_index.rs, project_indexer.rs,
type_inference.rs and internal Tree-sitter php_symbols extractor. The index owns
per-file declarations and reverse symbol references, plus candidate dependencies
for relinking when declarations appear/disappear without reparsing callers.
File replacement/deletion removes old edges and recomputes touched usage counts.
Disk JSON cache contains only disk facts; revision-bound buffer overlays stay in
memory. Startup reconciles mtime/size; watcher/save updates paths, watcher lag
reconciles metadata. Worker I/O/parsing/cache operations never run in editor frames.

File menu and ActionRegistry expose file.invalidate_caches, with an actual
confirmation modal, physical directory deletion and background rebuild/progress.
PHP completion uses declarative triggers, lexical variables/parameters, direct
new assignments and namespace/simple aliases; filters instance/static members and
visibility, ranks project results ahead of the bundled catalog. Ctrl/Cmd hover and
click/F12 navigate to local source; usages have code lenses, Alt+F7/menu, dark
four-column rows, filter/grouping, keyboard and mouse navigation and a separate
viewport button. Method/property/private-constant names with zero static references
are gray with a tooltip. Dynamic calls/Reflection/framework entry points are not
modelled. Inheritance/traits, chained return types, grouped imports and full PHP
flow typing remain outside this bounded resolver; Go/JS/TS partitions have no
extractor yet. Existing LSP remains available for unresolved definitions/hover.

Behavioral tests cover exactly one parse after changing one of 50 files; zero
parses on unchanged cache restart; cache invalidation/rebuild/deletion; unsaved
cache exclusion; references arriving before declarations, repeated calls, reference
removal/counts; local scopes, direct new, static/instance filtering and visibility.
Headless egui covers actual -> text activation, dot exclusion, unused color, Alt+F7,
usages keyboard navigation and actual confirmation-button clicks deleting a cache
sentinel and rebuilding. The previous DateTime::format completion regression now
uses the truly static createFromFormat, matching the new static-context contract.
Final verification results follow below.

Final native verification: workspace/all-features tests passed outside sandbox —
157 passed, 0 failed, 5 existing system/live tests ignored. Native workspace
all-targets/all-features Clippy with -D warnings, fmt, workspace release build and
git diff --check passed. The sandbox-only UI repeat hit existing mock TCP/SSH
PermissionDenied errors; the full unrestricted run passed. New editor completion
queries read one Rope line and retain absolute scope coordinates. LSP member
fallback remains available with member-kind filtering for unsupported local types.
Projects with no PHP files receive an explicit Finished event to stop progress.
No native GUI smoke or Windows/Linux GUI was run; separate viewport appearance
is covered only by API/headless behavior, not manual platform verification.

Default update-binaries.py rebuilt the macOS ARM64 executable and Nozhik.app.zip;
--check verified source fingerprints and payload checksums. The repository ignores
/.nozhik/cache/. No Windows build/Clippy or Windows target cache was created.

### PHP class usages, hierarchy and declaration-hover navigation correction

Fixed missing class-reference extraction for use/as/grouped imports, extends,
implements, type hints, new/instanceof, attributes/catch and static class scopes.
Class counts represent actual type references; member calls are not summed into
class counts. Parent/interface metadata resolves inherited methods/properties to
the declaring class, respects overrides and handles parent::. The PHP-specific
extractor also resolves $this->typedProperty receivers. No language settings or
universal grammar configuration was added.

Reverse edges retain actual resolved targets per file and hierarchy dependencies,
including missing ancestors. Changing a parent or removing/restoring an override
relinks existing caller facts; only edited files are parsed. Cache schema 2 causes
schema-1 indexes to rebuild automatically once, instead of preserving stale links
under unchanged mtimes. Valid schema-2 restarts still parse zero unchanged files.

Cmd/Ctrl-hover on a declaration opens its persistent, clickable usages window.
Releasing the modifier and moving to a row keeps it open; row selection wakes file
navigation and reveals the exact source name. A hover latch prevents immediate
reopening after dismissal and rearms on tab activation. Calls still navigate to
the actual declaration. Tests cycle declaration→usage→declaration for inherited
methods, aliases/classes and properties through real egui pointer press/release,
asynchronous file loading, exact caret bytes and visible target lines.

Added examples/php-symbol-navigation: interface, abstract base, Car/Truck,
ElectricCar (second inheritance level), Fleet and main.php. Fixture tests assert
exact counts: interface 4, base class 9, Car 6, Truck/ElectricCar 2 each, Fleet 1;
Vehicle.run 5, Truck.run 1, Vehicle.name 5, Vehicle.instances 2. Tests verify reverse
parse order, override removal/restoration, changing Car's parent and relinking
ElectricCar calls without parsing callers, cache restart/schema upgrade, comments/
strings exclusion, other type positions and cycle-safe malformed inheritance.
The binary script fingerprints examples too because tests embed these sources.

Final native verification: full workspace/all-features tests outside sandbox —
160 passed, 0 failed, 5 existing system/live tests ignored. fmt, native Clippy
workspace/all-targets/all-features with -D warnings, workspace release build and
git diff --check passed. Native macOS smoke used an isolated app bundle/settings
(AI off) and temporary copy of the fixture: checked base class=9, method=5, actual
five usage rows and row-click navigation to Fleet.php:23 with caret/reveal. Native
menu definition navigation also resolved the inherited call. Automated native
modifier-click synthesis did not reproduce a physical Cmd-hover; modifier behavior
is verified by the headless input regression. The test app exited via Cmd+Q.
User settings/project files and existing app instances were not changed.
Windows/Linux GUI and Windows builds were not run.

Default update-binaries.py rebuilt the macOS ARM64 executable/app archive;
--check verified the new source fingerprint and both payload checksums. Nested
.nozhik/cache directories are ignored too, so opening the example as a project
does not add cache data to Git or binary source hashes. No Windows target cache
was created.

### Language preferences and large-project indexing (2026-10-29)

Audit: the semantic extractor already parsed only PHP/PHTML; FileIndex separately
inventories all unignored names for file navigation. However unsupported-file
watcher events still persisted the full semantic cache and sent full snapshots,
and startup repeatedly linked after each parsed file using linear FQN lookup.

Added config::languages registrations and typed enabled_languages preferences,
Settings → Languages with live persistent PHP checkbox, and unavailable JS/TS,
HTML, YAML and Go entries. Old settings default to PHP; empty selection is allowed.
Unavailable IDs cannot start a backend. Disabled PHP drops services/editor semantic
state, skips cache traversal/reads and catalog loading, and disables code-style/
formatter behavior for that file. Text editing, filename search and independent AI
remain available. Re-enable keeps/reuses the disk cache and overlays open buffers.

Unified worker discovery/Changed/Overlay filters, fixed ignore behavior without
.git, made no-op events skip snapshot copies/persistence, and stopped rewriting
unchanged cache on startup. Initial/reconcile parsing now installs a batch before
linking once. FQN lookup is a derived hash table automatically rebuilt by cache
deserialization; serialization borrows large disk structures. Incremental edits
still parse exactly one file, including the existing 50-file regression.

Tests cover PHP/PHTML uppercase extensions, unsupported invalid-byte sources and
buffers, no-op cache mtime/bytes, disabled startup with invalid cache/source, future
Go-only selection, legacy IDs/settings and TOML/JSON persistence. A synthetic
501-PHP/500-JSON project verifies only 501 parses, all 500 cross-file inherited
references and one parse/one removed edge on edit; warm cached restart remains zero
parses. Headless real checkbox clicks verify disabled startup, enable/index, disable
with an open buffer, persisted empty selection, plain-text retention/format guard,
and re-enable without rewriting the cache. Languages settings are also checked on
small viewports with the existing Close/control visibility regression.

Final native verification: full workspace/all-features run outside sandbox —
165 passed, 0 failed, 5 existing system/live tests ignored. fmt, workspace native
Clippy all-targets/all-features with -D warnings, release workspace build and
 git diff --check passed. The first expanded viewport test found that introductory
text pushed the PHP control below the compact viewport; moving explanatory text
below the language toggles fixed it, and the full run passed. No manual native GUI
smoke was run for this iteration; real pointer actions/settings persistence and
small-window geometry are covered headlessly. Default update-binaries.py rebuilt
macOS ARM64 executable/app archive; --check verified source/payload hashes. Windows
builds/Clippy/GUI were not run and no Windows target cache was created.


### Read-only project cache audit and ignored-source references (2026-10-29)

Audited nozhove.com without writing its files/cache. Its schema-2 cache was populated
(432 PHP sources, 8,218 declarations, 20,299 resolved references), but unconditional
vendor exclusion omitted its explicitly autoloaded local framework. Dynamic routing
explains zero direct controller references; those counts were not artificially raised.

Added all-supported-source discovery, constructor promotion, cross-file typed
receiver/call-result linking, typed PHPDoc references and schema-3 cache migration.
Fixed invalidation snapshots advertising overlay revision gates without overlay facts.
Regression tests cover ignored vendor sources without Composer configuration, cold/warm cache,
invalidation with overlays, incremental return/property-type changes without caller
reparsing, PHPDoc coordinates and real headless UI import/declaration/usages clicks.

Per clarified user policy, ignore rules now affect tree coloring only: all supported
files in vendor/ignored/generated directories participate in semantic indexing, and
all non-internal names remain searchable. Git index membership overrides ignore
coloring for tracked files and their ancestor directories. Actual project tree loaded
read-only confirms vendor/desfpc/icecms2/Tools/FlashVars.php and its ancestor folders
are not marked ignored. Read-only classification uses the existing vcs libgit2 engine;
fs_model gains a one-way vcs dependency and no external Git requirement.

Full-source temporary-copy audit after policy clarification: all 607 PHP files indexed,
11,025 declarations and 37,014 resolved references. FlashVars has 71 references,
AbstractEntity 51 and Exception 1,267. Every reverse edge for those three classes
resolves back to the same declaration. The Messages import cycle and counts survive
Invalidate and warm restart. The original project remains read-only throughout.

Validation: native workspace/all-features tests passed — 169 passed, 0 failed,
5 existing live/system tests ignored. Native workspace Clippy all-targets/all-features
with -D warnings, rustfmt, diff whitespace checks and workspace release build passed.
Headless pointer navigation exercises import → vendor class → usages row → import;
no manual native window smoke was run. The sandboxed standalone watcher check timed
out; the full run with system watcher access passed. SHA-256 comparison verified
all 607 original PHP files and its existing cache unchanged (608 files total).

Default update-binaries.py rebuilt macOS ARM64 executable and signed app archive;
--check verified source fingerprint and both payload checksums. Windows builds,
Clippy and GUI were not run and no Windows target cache was created.


### Single-row editor tabs and viewport overflow (2026-10-29)

Replaced wrapped file/SQL controls with compact rounded tabs in one horizontal
scroll area. Fixed trailing down-arrow appears only on overflow; its popup derives
fully/partially hidden files from current scroll geometry, and selection reveals
the target. A trailing horizontal ellipsis menu provides Go to File (Files filter),
Close All Tabs through the unsaved-change gate, and Close Unmodified Tabs retaining
all dirty file buffers. Bulk file actions exclude separate SQL consoles.

Headless pointer/wheel regressions exercise one-row geometry, partial visibility,
left/right hidden tabs, selection/reveal, viewport resize, menu actions, clean-tab
removal and modified-buffer close confirmation.

Final native verification: workspace/all-features tests passed outside sandbox —
170 passed, 0 failed, 5 existing live/system tests ignored. The sandbox repeat
encountered PermissionDenied in existing local TCP mocks; unrestricted tests passed.
Native workspace Clippy all-targets/all-features with -D warnings, rustfmt,
git diff --check and workspace release build passed. Default update-binaries.py
rebuilt macOS ARM64 executable/app archive; --check verified source and payload
hashes. Native GUI appearance was not manually exercised. Windows builds/Clippy/GUI
were not run and no Windows target cache was created.


### Content-sized hidden-tabs popup (2026-10-29)

Replaced the fixed 240-point width/360-point height with measured label/row sizing.
The popup grows and shrinks with its currently hidden tabs and is capped at half
the central editor's width and height, including frame margins. Explicit inner
bounds avoid a remembered small egui Area clipping the list. Long names truncate
on one line at the cap, retain full-path tooltips, and long lists scroll vertically.
A headless regression checks short→long→resized→short geometry with themed fonts.


Verification: 171 native workspace/all-features tests passed, 0 failed, 5 existing
system/live tests ignored. Native workspace Clippy all-targets/all-features with
-D warnings, rustfmt, diff checks and workspace release build passed. Popup geometry
and existing activation/menu behavior were verified headlessly; no manual native
GUI smoke or Windows build/Clippy/GUI was run. Default update-binaries.py refreshes
macOS ARM64 executable/app archive only; source/payload checksum verification passed.

### Terminal header, clipboard selection and remembered height (2026-10-29)

Unified terminal tab painting with file tabs: one scrolling row, matching font,
padding, corners, active/hover state and per-session close hit area. Inline +
creates a shell; fixed vertical dots (drawn geometry, avoiding a missing font
symbol) offer Close All, followed by a Hide minus that preserves sessions.
Opening an empty panel creates one local shell; closing/exiting sessions while
visible does not respawn them until the panel is reopened. No-project shells
use the application's current directory.

Initial bottom height is 40% of the viewport. Dragged heights may exceed the
editor's remaining height and survive Hide/reopen; the release frame participates
in splitter state persistence. Existing stored layouts remain respected.

Per-session mouse/Shift-click selection uses cell boundaries and handles Unicode,
wide characters, reverse and multiline ranges. Ordered pointer-event replay
covers native drags whose press/move/release arrive in one frame. Context menu,
keyboard clipboard events and native Edit actions target the focused terminal;
paste retains bracketed-paste negotiation. Cut deletes a selected live cursor-row
range with line-editor input; history/multiline/alternate-screen output is copy-only.
Copy with a selection does not interrupt the shell; plain Ctrl+C without a
selection remains available. Disabled/unfocused views ignore clipboard input.

Validation: 175 native workspace/all-features tests passed, 0 failed, 5 existing
system/live tests ignored. Full tests ran with system access after sandbox TCP
PermissionDenied failures. Native workspace Clippy all-targets/all-features with
-D warnings, rustfmt, workspace release build and git diff --check passed.
Regressions cover actual egui drag/fast drag, Unicode clipboard output, exact
bracketed paste and live-line cut bytes, focus/modal suppression, real header
menu clicks, Close All, reopening an empty panel and actual splitter resizing.
Native macOS smoke on an isolated temporary project/settings (AI/languages off)
confirmed appearance, the drawn vertical menu icon, automatic shell creation,
system-clipboard command paste/execution and visible mouse selection. Final test
process was explicitly stopped; native menu/bulk actions are covered headlessly.
Default update-binaries.py rebuilt macOS ARM64 executable/app archive; --check
verified current source and payload hashes. Windows build/Clippy/GUI were not run.
