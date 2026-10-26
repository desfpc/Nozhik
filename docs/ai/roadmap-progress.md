# Roadmap и прогресс

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
| 6.1 | БД: подключения, схема, SQL-консоль, результаты | Не начат |
| 7 | Task runner, запуск и отладка | Не начат; терминал реализован в 6, Git в 3.4 |
| 8 | Развитие API расширений, темы, marketplace | Фундамент плагинов/keymap вынесен в этап 4 |
| 9 | Профилирование, accessibility, безопасность, упаковка и релизы | Не начат |

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
