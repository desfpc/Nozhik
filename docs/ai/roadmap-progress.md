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
| После релиза | Активация платного ПО по ключу/email, три активированных устройства | Не реализована; спецификация в [paid-software-licensing.md](paid-software-licensing.md) |

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
берётся из Git config; hooks/GPG/rebase/submodule lifecycle не реализованы; merge UI добавлен ниже (2026-10-29).
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
В исходном Phase 8 не было editable cells (добавлены в v0.1.8), server-side pagination, SQL completion/LSP, persisted console
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
runner/debugger, marketplace, full accessibility, signed installers, SQL pagination, многие vendor SQL types и persistent sessions остаются вне scope.
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


### PHP this/trait completion and stable typing presentation (2026-10-29)

Added nested trait composition to member lookup/completion with class→traits→parents
precedence, inherited visibility and private trait members in the consuming class.
Schema 4 refreshes old caches. Retained completion context maps current caret positions
back to its indexed Rope while the new overlay is pending, so typing `$this->` does
not temporarily lose the class context. Existing trait-adaptation limitations remain.

Editor presentation retains and rebases syntax colors, unused names and code-lens
positions before painting changed text. Current-revision worker results replace them;
navigation/diagnostics keep their revision gates. This removes the empty-color and
missing-lens frames that made the document flash and jump. No synchronous parser or
full-document String allocation was introduced. Completion popup uses measured width,
24-point rows, up to nine visible suggestions and explicit Area bounds that grow and
shrink across successive queries.

Regressions cover multi-level inheritance, aliased/nested traits, overrides, private
visibility, real multi-event `$this->` typing/Tab acceptance without a worker reply,
retained colors/lens geometry, Unicode edits/Undo and repeated short/long popup sizing.

Validation: native workspace/all-features tests passed with system access; targeted
UI reruns and a multi-chunk Unicode replacement regression passed after the final
presentation changes. Native workspace Clippy all-targets/all-features with
-D warnings, rustfmt, diff whitespace checks and workspace release build passed.
Popup geometry and typing behavior were verified headlessly; no manual native GUI
smoke or Windows build/Clippy/GUI was run. Default update-binaries.py rebuilt the
macOS ARM64 executable and signed app archive; --check verified the source fingerprint
and both payload checksums. No Windows target cache was created.


### Unified left tool dock (2026-10-29)

Removed the Structure button, placeholder window and icon. Database and
Notifications now share the mutually exclusive left dock with Project, Git and
SSH; repeated clicks hide the active tool. Removed the right rail, right dock
and application right-tool state. Database menu actions, New Connection and SQL
Console select the left Database dock. The legacy right layout proportion remains
only for compatibility with existing layout.json files.

Validation: native workspace/all-features tests, native workspace Clippy with
all-targets/all-features and -D warnings, rustfmt, workspace release build and
git diff --check passed. Existing headless UI tests cover shell/menu rendering,
database explorer/console actions and left splitter persistence. No manual native
GUI smoke or Windows build/Clippy/GUI was run.
Default update-binaries.py rebuilt the macOS ARM64 executable and signed app
archive; --check verified current source and payload hashes. No Windows target
cache was created.


### Git reference layout, branch actions and merge resolution (2026-10-29)

Added a dedicated Git header with Current Repository/Current Branch/Fetch-Pull-
Push-Publish origin controls, context menus, vector arrows/status icons, grouped
local/remote branch chooser, New Branch and dirty-switch Leave/Bring Changes
cards. Git keeps its own persisted sidebar width, compact fixed rows, bottom
commit form, filter modes and multi-select file context actions (discard/ignore/
extension/include/exclude/copy/open/reveal). Mutating requests gate controls;
background reads no longer dim or disable them. The diff remains visible while
loading, caches eight targets, retains scroll IDs and rejects stale replies.

Added worker-only branch stashes including staged/unstaged/untracked restoration,
remote tracking checkout, in-memory merge preview, clean-start merge commits,
conflict snapshots/markers, editor opening, Continue Merge and confirmed Abort.
Continue checks saved resolution again and cannot be bypassed just by staging
conflict markers externally. Binary/deleted/structural conflicts need explicit
index resolution. Hooks/GPG/rebase/submodules remain outside this feature.

Behavioral regressions cover conflicting/nonconflicting merges, preview without
mutation, two-parent commits, abort preserving unrelated untracked files, dirty
stash/index restoration, safe refusal of overlapping checkout, remote tracking,
bulk discard/extension ignore, staged-marker gates, actual egui branch/create/
switch/merge/continue/menu clicks, multi-selection and stable sparse-row/footer/
diff geometry. Validation results follow after the final native checks.


Final validation: 187 native workspace/all-features tests passed, 0 failed, 5
existing system/live tests ignored. The sandbox-only attempt failed on existing
local TCP mock permissions; the unrestricted full run passed. Final targeted
Git UI tests, native workspace/all-targets/all-features Clippy with -D warnings,
rustfmt, workspace release build and git diff --check passed. A staged-markers
regression additionally verifies the committed blob contains the saved resolution,
not an older staged marker version. Git modals block global/commit shortcuts.

Native macOS smoke used an isolated temporary app/settings/project (AI/languages
off): branch picker, Leave Changes stash, conflicting merge preview/start,
Open in Nozhik, actual editor replacement/save, disabled→enabled Continue Merge
and resulting two-parent merge commit. The final packaged executable also verified
layout, vector icons, filter/file menus, Switch Branch cards and automatic stash
restoration on return. The test app was explicitly closed. The real workspace
repository was not switched, merged, committed or pushed. Network behavior is
covered against a disposable local bare origin; no real origin push was run.
Default update-binaries.py refreshed macOS ARM64 executable and signed app archive;
--check verified the source fingerprint and both payload checksums. Windows builds,
Clippy and GUI were not run; no Windows target cache was created.

### Git History columns and selectable diff (2026-10-29)

History now paints commits as full-width 56-point hit areas with consistent padding,
hover/active backgrounds and secondary author/time lines. Changed files occupy a
separate resizable second column with full-width 32-point rows, status icons and
independent vertical scrolling. Commit metadata spans above the files/diff area;
selecting a commit invalidates its outstanding details/diff requests.

Diff retains virtualized painting and stores Unicode-scalar selection endpoints.
Ordered pointer replay supports forward/reverse multiline drags, including a full
drag arriving within one frame; dragging content no longer pans the ScrollArea.
Both scrollbars and wheel axes remain enabled. Copy excludes gutters/patch markers
and works through Cmd/Ctrl+C, the context menu and native Edit Copy; Cmd/Ctrl+A
selects the displayed patch. File replacement clears selection. Patch width is
measured once per file/font to keep horizontal range stable across vertical scroll.
Copy is available without an open editor because Git is a read-only text surface.

Validation: 189 native workspace/all-features tests passed, 0 failed, 5 existing
system/live tests ignored. Sandbox mock TCP permissions required running the full
suite with system access. Native workspace/all-targets/all-features Clippy with
-D warnings, rustfmt, whitespace checks and workspace release build passed.
New pointer regressions cover padded commit/file clicks, separate column geometry,
Unicode multiline/reverse/one-frame selection, clipboard output, Select All, both
scroll axes and selection reset. Native macOS smoke used an isolated temporary
project/settings and verified three columns, full-width highlights, visible text
selection without panning, context Copy and Cmd+C by pasting the selected fragments
into a temporary commit-description field. No commit was made; the app was closed.
Default update-binaries.py refreshes only macOS ARM64 executable/app archive and
--check verifies source/payload hashes. Windows builds/Clippy/GUI were not run.

### Git diff syntax and expandable context (2026-10-29)

Changes and History share PHP/PHTML Tree-sitter colors and configured syntax
palettes. Independent background parsing of full old/new Git snapshots preserves
multiline lexical context and colors removed lines from the old version. Shared
language availability/enabled preferences gate parsing; unsupported/partial files
stay plain. No formatter runs and no source files are rewritten.

VCS supplies optional complete context alongside its existing bounded patch.
DiffView starts with changes plus three adjacent rows. Gaps expose vector up/down
arrows as applicable, revealing 20 rows at a time until the file boundary or the
next visible region. Full-context limits retain the original partial patch.
Identical normalized polling replies retain expanded context, colors and selection;
actual replacement/expansion clears obsolete row selection coordinates.

Validation: 193 native workspace/all-features tests passed, 0 failed, 5 existing
system/live tests ignored. Full tests required system access for existing TCP
mocks after sandbox PermissionDenied failures. Native workspace Clippy with
all-targets/all-features and -D warnings, rustfmt, whitespace checks and workspace
release build passed. Regressions exercise actual egui up/down clicks, repeated
expansion/EOF, polling retention, Unicode drag/copy/scroll, multiline comments,
old/new token colors, plain palette/disabled language and worktree/index/historical
snapshot isolation. No manual native GUI smoke or Windows build/Clippy/GUI was run.
Default update-binaries.py rebuilt macOS ARM64 executable and signed app archive;
--check verified current source fingerprint and both payload checksums. No Windows
target cache was created.

### Git History commit actions and origin badges (2026-10-29)

Added stable SHA-based Shift range/Cmd-Ctrl toggle selection and right-click
selection targeting. Single-commit menus implement Amend, soft/mixed/hard Reset,
detached Checkout, Reorder, first-parent Revert, branch/tag creation, Cherry-pick,
SHA/tag copying and validated View on GitHub. Multi-selection menus implement
ordered Cherry-pick, contiguous Squash, Reorder and SHA copying. Concrete dialogs
capture HEAD and branch, retain errors, accept summary/body or branch/tag names,
filter local cherry-pick destinations, create new destinations and move selected
commits with buttons/Up/Down (Enter confirms). History walks first parents,
displays tag badges and explicitly labels the last-clicked details for groups.

Worker preflights replay/revert patches in memory and refuses conflicts without
changing refs, worktree or the disk index. Staged-only amend retains the original
author and unstaged work. Merge replay uses mainline 1 and flattens the merge;
root replay/revert/squash are currently unavailable. No forced push or interactive
replay conflict continuation is introduced. Existing editor buffers remain retained.

Origin now displays Fetch/Publish/Pull/Push/Pull & Push with independent down/up
count badges. Combined sync fetches then fast-forwards or merges divergence and
pushes without force. Conflicts enter the existing merge resolution UI and leave
push explicit after Continue. Counts reflect fetched tracking refs, not an
implicit periodic network request. Badge/button arrows are vector geometry: native
smoke caught missing Unicode arrow glyphs and the final build fixes them.

Validation: full native workspace/all-features tests with system access passed —
201 passed, 0 failed, 5 existing system/live tests ignored. The sandbox attempt
failed on existing local TCP mock permissions. Final Git UI rerun passed after
the vector-arrow change; native workspace/all-targets/all-features Clippy with
-D warnings, rustfmt, workspace release build and diff whitespace checks passed.
Temporary repository tests cover actual pointer selection/menu targeting, dialog
cancel/revert/cherry-pick/squash/reorder, all reset modes, stale HEAD/same-SHA branch
gates, author/index retention, non-commit tags, first-parent merge replay, conflict
preflight and local-bare-origin combined sync/conflict-without-push.

Native macOS smoke on a separate temporary project/settings (AI/languages off)
verified full-width selected rows, Shift group highlighting, group context menu,
Squash summary/body/button layout and cancellation. A final packaged app launch
verified Pull & Push with two correctly drawn directional/count badges. Test
processes were stopped. No real project branch/commit/push was performed.
Default update-binaries.py rebuilt macOS ARM64 executable and signed app archive;
--check verified source fingerprint and payload checksums. Windows build/Clippy/GUI
were not run and no Windows target cache was created.

### Git History selected-file union and read-only file menu (2026-10-29)

SelectionDetails now returns owned metadata and a sorted union of files from all
selected commits. Each path appears once and retains its selected contributor IDs.
Per clarified user behavior, each file opens the ordinary diff from its newest
selected contributing commit; skipped/intermediate commits are not composed.
A count badge and tooltip explain repeated contributors. This preserves existing
PHP syntax, expandable context, immutable SHA diff caching and text selection.
Changing/toggling/pruning the selection invalidates stale requests; deselecting all
clears details/diff. Union includes selected additions later deleted or renamed.

History file menus offer Reveal in File Manager, Open in Nozhik, Open with Default
Program, Copy File Path, Copy Relative File Path and View on GitHub. Local actions
use the current saved worktree and are disabled when it is absent; copy remains
available. Worker-produced GitHub links encode path bytes and use immutable commit
IDs, or the first parent for deleted paths. Unsupported origins disable browser
actions. Historical inspection never stages, discards, restores files or moves refs.

Validation: 206 native workspace/all-features tests passed with system access,
0 failed, 5 existing system/live tests ignored. Native workspace/all-targets/
all-features Clippy with -D warnings, rustfmt, workspace release build and diff
whitespace checks passed. New disposable-repository regressions cover nonadjacent
selection, duplicate paths, newest-selected patches, skipped-only files, unchanged
index/worktree/HEAD, root/deletion/rename metadata and encoded historical links.
Actual headless egui clicks verify file-menu contents, right-click targeting,
clipboard output, editor-open requests, browser URLs, disabled missing-file/origin
actions, count badges, selection toggles and obsolete response rejection.

The final packaged native macOS app launched with isolated temporary project and
settings; CUA returned noWindowsAvailable for click actions, so no manual native
menu interaction is claimed for this iteration. The test process was stopped.
Default update-binaries.py rebuilt macOS ARM64 executable and signed app archive;
--check verified current source fingerprint and both payload checksums. No real
repository branch/commit/push or Windows build/Clippy/GUI was performed.


### Project tree header actions (2026-10-29)

Added Select Opened File, Expand Selected, Collapse All and Hide to the fixed
Project header, using vector target/chevron/minus icons, English tooltips and
accessibility labels. Directory rows now visibly select their paths. File reveal
opens ancestors lazily and scrolls to the active local project's file; SQL, SFTP
and outside-project documents disable the action. Recursive expansion uses the
existing filesystem worker and at most 16 outstanding directory requests, leaves
other branches lazy and includes empty/ignored directories under the existing
symlink policy. Collapse cancels pending intents and late replies cannot reopen
nodes. Hide/reopen retains selection, cached tree and open documents. The Project
dock has enough minimum width for its header controls.

Validation: native workspace/all-features tests passed with system access —
207 passed, 0 failed, 5 existing system/live tests ignored. The sandbox attempt
was blocked by existing local TCP mock permissions. Final UI/all-features rerun,
native workspace/all-targets/all-features Clippy with -D warnings, rustfmt,
workspace release build and git diff --check passed. New headless pointer tests
cover actual toolbar clicks and directory selection, lazy ancestor loading,
scrolling to an offscreen active file, recursive/empty subtree expansion,
untouched lazy siblings, cancellation with outstanding replies, disabled actions
and Hide/reopen retention. No manual native GUI smoke or Windows build/Clippy/GUI
was run. Default update-binaries.py rebuilds the macOS ARM64 executable and signed
app archive; --check verified the current source fingerprint and both payload
checksums. No Windows target cache was created.


### Project launcher, cloning and version 0.1.1 (2026-10-29)

Default launch shows a compact Projects/About screen with existing logo, Cargo
version, searchable persistent project MRU, Open Folder and Clone Repository.
Successful folder/clone opening expands the same viewport into the IDE; failures
retain the launcher. Explicit CLI project paths retain direct-opening behavior.
Removed implicit cwd opening. About links to https://github.com/desfpc/Nozhik.
Workspace, package lockfile, UI and app bundle now share 0.1.1; AGENTS.md records
one patch increment per follow-up change/commit, with 0.1.2 next.

Clone URL authentication supports public/helper/agent, HTTPS token, login/password,
and SSH key/passphrase. GitHub.com PAT connection validates GET /user and loads
all available user repositories with pagination, filtering and private badges.
No Enterprise or OAuth client registration. GitHub account passwords cannot be
used for Git. Secrets are session-only Zeroizing strings, excluded from URLs,
settings, Git config and error messages. Credential redirects are refused; default
TLS/SSH host verification remains enabled. Cloning stages a completed checkout
off-thread, refuses existing destinations and opens it automatically. Cancellation
suppresses late replies; network transport controls how quickly it completes.
Forms scroll on small viewports. Later IDE fetch/push still use system credentials.

Validation: native workspace/all-features tests passed — 211 passed, 0 failed,
5 existing system/live tests ignored. Sandbox execution was blocked by existing
local TCP mocks; the full run with system access passed. Regressions cover actual
launcher project-row clicks/search, invalid-project retention, successful viewport
transition, local clone content/origin, existing-destination protection/cancellation,
credential selection, secret clearing, cancelled completion rejection and mock
GitHub account authentication/pagination (101 repositories, bearer header). The
existing IDE splitter test now opens a real temporary project before checking docks.
Native Clippy workspace/all-targets/all-features with -D warnings and rustfmt passed.

Native macOS smoke used an isolated temporary app bundle/settings/project history,
with AI/languages off: verified logo/version/MRU, About link, clone dialog and actual
local URL clone → expanded IDE with cloned project tree. Checkout contents and MRU
were verified on disk. Quit was issued to the test app. No real GitHub account,
private remote authentication, remote push or Windows build/Clippy/GUI was exercised.
Final workspace release build and default update-binaries.py/--check refresh only
macOS ARM64 executable and signed app archive; no Windows cache was created.


### Обновление логотипа (v0.1.2, 2026-10-29)

Иконка приложения и логотип стартового окна заменены на предоставленный
`docs/img/logo.png`. `app/assets/source.png` сохраняет исходный PNG без изменений;
мастер PNG, viewport PNG, Windows ICO и macOS ICNS получены с сохранением
прозрачности и пропорций, без дополнительных полей. Общий ресурс стартового
окна и viewport уже подключён; изменения Rust UI не потребовались.
Версия workspace/Cargo.lock и bundle — 0.1.2, следующая доработка — 0.1.3.

Проверена декодируемость PNG, всех семи ICO и всех ICNS представлений;
визуально просмотрен логотип в размерах 160, 56 и 32 px на тёмном фоне.

Проверки: cargo fmt, native workspace Clippy all-targets/all-features с
-D warnings, workspace release build и git diff --check прошли. Полный
workspace/all-features test вне sandbox: 211 passed, 0 failed,
5 ignored. Первый sandbox-прогон блокировал существующие TCP mocks.
update-binaries.py обновил только macOS ARM64 executable и подписанный app
archive; --check подтвердил source fingerprint и SHA-256 payload. Проверены
целостность ZIP, совпадение упакованной ICNS и версия bundle 0.1.2.
Ручной запуск GUI и Windows build/Clippy не выполнялись.

### Несколько проектов в одном окне (v0.1.3, 2026-10-30)

Workspace координирует независимые NozhikUi-сессии: отдельные EventBus,
файловый worker/watcher, дерево, буферы, Git, терминалы, SQL, SSH, настройки
и layout. При переключении состояния и несохранённые правки сохраняются;
неактивные workers продолжают принимать ответы, но не показывают диалоги.
Отложенное открытие файла привязано к исходному проекту. Закрытие использует
существующий Save All / Discard / Cancel gate, включая неактивные проекты
при выходе из IDE; после закрытия последнего проекта доступен launcher.

Верхняя строка содержит имя активного проекта и меню Open / Clone / Open Projects /
Recent Projects с цветными иконками, путями, отступами и скруглениями по референсам.
Ниже — равномерные горизонтальные вкладки: активная тёмная, hover светлее,
крестик при наведении на любую вкладку. Drag-and-drop сохраняет активную сессию
и явно запрашивает перерисовку после смены порядка. macOS использует прозрачный
native titlebar с системными кнопками и перетаскиваемым фоном верхней строки.

Новый проект получает Config::default, AI debounce — 1000 мс. Изменения сохраняются
worker в `.nozhik/settings.toml`, layout — в `.nozhik/layout.json`; диалог явно
показывает имя и путь проекта. Общий MRU остаётся рядом с глобальным config.
Повреждённый project config не перезаписывается и не заменяет открытые сессии.
Watcher исключает `.nozhik`, чтобы атомарная запись внутренних файлов не запускала
повторные rescans. Версия Cargo/lockfile/UI/bundle — 0.1.3, следующая — 0.1.4.

Три новых headless egui-регрессии проверяют реальные клики меню/вкладок/крестика,
перетаскивание в обе стороны (в том числе целиком за один кадр) с запросом repaint,
сохранение буферов/SQL/терминального состояния/дерева, раздельные настройки через
настоящий slider и settings worker, повторное открытие, canonical deduplication,
ошибки пути/config, отмену/сохранение/отказ от правок при закрытии, проверку
неактивных dirty-буферов при выходе и отложенные диалоги неактивных проектов.

Верификация: полный native workspace/all-features test — 214 passed, 0 failed,
5 existing system/live tests ignored. Для TCP mocks полный прогон выполнен
с system access после sandbox PermissionDenied. Native Clippy workspace/all-targets/
all-features с -D warnings, rustfmt, workspace release build и diff whitespace
check прошли. update-binaries.py обновляет только macOS ARM64 executable и
подписанный app archive; --check проверяет source fingerprint и payload SHA-256.

Нативная macOS-проверка использовала отдельную временную app-копию с собственным
bundle ID, временными Alpha/Beta-проектами и отключёнными AI/языками. Проверены
меню, открытие второго проекта, видимая перестановка вкладок, сохранение выбора,
подсветка hover/крестик, переключение деревьев, project scope в Settings и закрытие.
Проверка поймала отсутствие repaint после быстрого drag; оно исправлено и
повторно проверено в нативном окне. Тестовое приложение закрыто. Реальные
пользовательские настройки, проекты и credentials не менялись. Windows build,
Clippy и GUI, commit и push не выполнялись.

### Project tab scrolling and live dragging (v0.1.4, 2026-10-30)

Removed the project row's scrollbar entirely, including hover expansion. A bounded
28-point ScrollArea inside the fixed 32-point row uses AlwaysHidden and accepts
only mouse-wheel/trackpad scrolling; overflow adds compact left/right vector
buttons. Width calculation uses the viewport instead of the scroll canvas.

Project dragging now reorders while primary is held and crosses neighboring tab
midpoints, rather than waiting for release. Gesture identity uses the project path
across index changes, preserving the selected session when dragging an inactive
project. Repeated forward/backward moves and same-frame press/move/release work;
close targets do not start drags, and release does not activate the moved project.
Every reorder requests a repaint. Workspace/Cargo.lock/UI/bundle are 0.1.4;
AGENTS.md records 0.1.5 next.

Validation: native workspace/all-features tests — 216 passed, 0 failed, 5 existing
system/live tests ignored, run with system access for local TCP mocks. New actual
egui input regressions cover held drag through multiple neighbors and back before
release, active-project retention, 20-tab overflow, both arrow buttons, horizontal
and vertical wheel input, stable viewport geometry under hover, and removal of
arrows/reset of offset when tabs fit again. Native workspace Clippy with
all-targets/all-features and -D warnings, rustfmt, workspace release build and
git diff --check passed. The initial build ran out of disk space; deleting only
the regenerable target/debug/incremental cache freed space for successful checks.

An isolated copy of the packaged macOS app with a separate bundle ID and temporary
Alpha/Beta projects (AI/languages disabled) verified no project scrollbar, real
native dragging in both directions, updated tab order and retained Beta selection.
The test app was quit. Default update-binaries.py rebuilt only macOS ARM64 executable
and signed app archive; --check verifies source and payload checksums. Windows
build/Clippy/GUI, commit and push were not run.


### Terminal / Problems в левой полосе (v0.1.5, 2026-10-30)

Горизонтальная нижняя строка переключателей удалена. Внизу левой полосы
добавлены квадратные 28 px кнопки с векторными иконками: Problems над Terminal.
Project, Git, Remote, Database и Notifications остаются вверху. Полоса создаётся
раньше нижнего dock: терминал и Problems расположены справа от неё, под деревом
и редактором. Пустое содержимое Problems больше не уменьшает выбранную высоту
dock при переключении. Существующие lazy PTY, terminal focus, hide/reopen и
сохранение размера продолжают работать. Problems остаётся прежним placeholder.
Версия workspace/lockfile/UI/bundle — 0.1.5; следующая в AGENTS.md — 0.1.6.

Новая headless egui-регрессия использует реальные pointer events при размерах
1000×800 и 800×600: открытие Terminal, переключение Problems, повторный клик
для сворачивания, reopen, положение верхней кнопки Project, полная высота полосы
и отсутствие перекрытия dock с полосой/деревом. Прежний resize/hide/reopen тест
сохранён. Нативная macOS-проверка на изолированной временной app-копии с отдельным
bundle ID и временным проектом подтвердила расположение и переключение кнопок,
полную высоту левой полосы и терминал справа от неё; тестовое приложение закрыто.
Реальные проекты и настройки не менялись.

Верификация: полный native workspace/all-features test — 217 passed, 0 failed,
5 existing system/live tests ignored; запуск с system access для локальных
TCP mocks. Native Clippy workspace/all-targets/all-features с -D warnings,
rustfmt, workspace release build и git diff --check прошли. Windows build,
Clippy и GUI, commit и push не выполнялись.

Локальный macOS ARM64 executable и подписанный app archive обновлены через
`update-binaries.py`; `--check` подтвердил source fingerprint и SHA-256 payload.
Финальная временная native app-копия повторно проверена и закрыта.


### Исправление Git toolbar (v0.1.6, 2026-10-30)

Убрана строка Current Repository с дублирующими именем проекта и иконкой папки.
Git toolbar и merge banner перенесены после левой полосы инструментов: теперь
Git располагается справа от неё и не сдвигает верхние кнопки вниз. Toolbar
сам задаёт horizontal layout; раньше основной shell вызывал его в вертикальном
Ui, из-за чего branch/remote controls выходили за 50 px и создавали пустую полосу.
Выбор ветки и сетевые действия занимают один ряд над Changes/History. Fetch origin
сохраняется видимым при Pull/Push/Publish; контекстное сетевое меню сохранено.
Версия workspace/lockfile/UI/bundle — 0.1.6, следующая — 0.1.7.

Новая shell egui-регрессия при 1000×800 и 800×600 проверяет общую верхнюю границу
rail/toolbar, toolbar справа от rail, ровно 50 px высоты, dock сразу под toolbar,
видимость branch/Fetch и отсутствие Current Repository. Pointer-driven Git
регрессия дополнительно проверяет видимый Fetch для ahead/behind состояний.

Проверки: полный native workspace/all-features test — 218 passed, 0 failed,
5 existing system/live tests ignored; системный доступ использован для локальных
TCP mocks. Все 11 Git interaction tests прошли также после финального refactor.
Clippy workspace/all-targets/all-features с -D warnings, rustfmt, workspace
release build и git diff --check прошли. Нативное окно проверено с отдельным
bundle ID и временным Git-проектом: нет повторного заголовка/пустой полосы,
rail не сдвигается, видны branch/Fetch/Publish, History работает. Клик Fetch
в локальный временный bare origin завершился успешно (Last fetched just now).
Тестовое приложение закрыто; реальные проекты/настройки не менялись.
macOS ARM64 executable и подписанный app archive обновлены update-binaries.py;
--check подтвердил source fingerprint и payload checksums. Windows build/Clippy/GUI,
commit и push не выполнялись.


### Единые цвета проектов и отступы вкладок (v0.1.7, 2026-10-30)

Welcome, заголовок активного проекта и open/recent project menu используют
единый painter `project_badge`. Цвет выбирается по первым трём символам имени
без учёта регистра из 12 ярких оттенков, независимо от пути и порядка проектов.
Примеры leadforms, nozhove.com, forms-analytics, mf-lf и antifraud получают
различные оттенки. Под вкладками добавлен отступ, симметричный верхнему: 4 px
с каждой стороны 28 px viewport, общая высота строки 36 px. Current File убран.
Workspace/lockfile/UI/bundle — 0.1.7, следующая версия в AGENTS.md — 0.1.8.

Внутренний ряд ScrollArea выровнен через horizontal_top: прежнее центрирование
смещало вкладки на 5 px вниз и выталкивало нижний край за viewport. Существующая
проверка overflow дополнена проверкой верхней/нижней границы каждой вкладки.

Проверки: 218 passed, 0 failed, 5 existing system/live tests ignored в полном
native workspace/all-features прогоне с system access для локальных TCP mocks.
Clippy workspace/all-targets/all-features с -D warnings, rustfmt, workspace release
build и git diff --check прошли. Нативная macOS-проверка на отдельной временной
app-копии с временными проектами подтвердила одинаковые пять цветов в welcome,
menu и header, отсутствие Current File и видимый нижний отступ вкладок после
исправления выравнивания. Приложение закрыто, реальные настройки не менялись.
Windows build/Clippy/GUI, commit и push не выполнялись.

macOS ARM64 executable и подписанный app archive обновлены update-binaries.py;
--check подтвердил source fingerprint и payload checksums.


### Объекты таблиц и отложенное редактирование данных (v0.1.8, 2026-10-30)

Database Explorer показывает columns, keys, foreign keys и indexes с количеством
объектов, типами/PK, определениями ограничений и ссылками. Каталоги SQLite,
PostgreSQL и MySQL/MariaDB получают нативные метаданные и DDL. Двойной клик и
Open Table открывают отдельный TableEditor в общей полосе вкладок; повторное
открытие сохраняет текущую сессию и правки. Go to DDL открывает SQL-вкладку
с редактором, подсветкой и возможностью сохранения файла.

Ячейки редактируются локально. Add Row использует DEFAULT для незаданных ячеек;
контекстное меню позволяет задать NULL. Номера строк поддерживают обычный выбор,
Cmd/Ctrl-toggle и Shift-range. Delete Rows только помечает выбранные строки.
Revert сбрасывает пакет, Preview Pending Changes показывает BEGIN/DML/COMMIT,
Submit выполняет те же statements в SQLx-транзакции и перечитывает исходную
вкладку. Три действия активируются при наличии правок. Reload защищён от потери
неотправленных изменений; Cancel Running Statements использует токен запроса.

WHERE включает исходные значения, в том числе старый PK, и проверяет ровно одну
затронутую строку. Конфликт, неоднозначность строк без PK или ошибка ограничения
откатывает весь пакет и сохраняет локальные правки. SQLite VM отменяется через
progress handler, серверное соединение выводится из пула после Submit. Начавшийся
COMMIT дожидается результата. MySQL Submit требует InnoDB; views доступны для
чтения. Save All не отправляет DML; закрытие с правками проходит Discard/Cancel.
Версия workspace/Cargo.lock/UI/bundle — 0.1.8, следующая — 0.1.9.

Новые SQLite-регрессии проверяют отсутствие записи до Submit, update/insert/delete,
изменение PK, DEFAULT/NULL/binary/Unicode/кавычки/NUL, извлечение ключей/index/FK/DDL,
Revert, откат целого пакета при ошибке/конкурентной правке/неоднозначной строке,
редактирование уникальной строки без PK и отмену долгого триггера с откатом.
Headless egui использует реальные pointer/key events: двойной клик по дереву,
дедупликация таблицы, группы объектов, ввод в ячейку, Preview/Cancel/Revert,
Submit обновления/вставки/удаления, dirty-close gate и отдельная DDL-вкладка.

Верификация: полный native workspace/all-features test — 222 passed, 0 failed,
5 existing system/live tests ignored. Native Clippy workspace/all-targets/all-features
с -D warnings, rustfmt и git diff --check прошли. После нативной проверки убраны
неподдерживаемые шрифтом символы Revert/Submit/PK, добавлены границы ячеек и
подсветка выбранных строк; открытие preview и изменения dirty явно запрашивают
repaint. Pointer-driven table regression повторно прошла после этих правок.

Нативная macOS-проверка использовала отдельную временную app-копию с отдельным
bundle ID, временным проектом/SQLite-файлом, отключёнными AI/языками. Проверены
подключение, дерево, двойной клик, редактирование числовой ячейки, DML Preview
и Submit с автоматическим перечитыванием; фактическое значение дополнительно
проверено через SQLite на диске. Реальные проекты, подключения и настройки не
менялись. PostgreSQL/MySQL/MariaDB live tests и Windows build/Clippy/GUI не
запускались; Docker daemon в этой сессии недоступен. Commit/push не выполнялись.

Повторная нативная проверка подтвердила видимую сетку/выделение строки, нормальные
подписи Revert/Submit/PK, появление Preview по первому клику, возврат исходного
значения через Revert и отдельную DDL-вкладку с подсветкой SQL. Тестовое окно
закрыто. Workspace release build прошёл; update-binaries.py пересобрал только
macOS ARM64 executable и подписанный app archive, --check подтвердил source
fingerprint и SHA-256 payload. Windows-кэш не создавался.

Финальный полный прогон после всех правок: 222 passed, 0 failed, 5 ignored.
Повторный sandbox-прогон UI блокировал четыре существующих TCP mock-теста с
PermissionDenied; полный workspace/all-features прогон с system access прошёл.


### Таймаут подключения и редактирование профилей БД (v0.1.9, 2026-10-30)

Исправлена регрессия v0.1.8: Connect ждал полный catalog crawl колонок, ограничений,
индексов и DDL каждой таблицы всех видимых namespaces. На удалённой БД это
исчерпывало общий timeout, не показывая дерево. Теперь Connect запрашивает только
имена, детали загружаются по требованию отдельными Describe tasks с timeout 20s.
Ошибки отдельных таблиц локализованы, доступны Retry; ошибка keys/DDL не мешает
читать колонки и данные. Test Connection проверяет handshake/authentication;
Connect различает transport/auth timeout и catalog timeout. Connecting…/готовый
Database Connected отделены от SSH transport ready. Запросы деталей дедуплицируются,
поздние ответы после disconnect/refresh/update игнорируются.

Edit Connection… доступен для Offline/Online профилей. Форма с прокруткой открывает
все существующие поля; Cancel сохраняет исходный JSON. Update заменяет ожидаемую
запись на прежней позиции и сохраняет другие профили. Duplicate/stale/corrupt
ошибки оставляют файл/форму. Saved credentials сохраняют прежний opt-in и test gate,
сессионный пароль того же endpoint сохраняется при пустом поле. После Save старый
pool отменяется; открытые вкладки сохраняют результаты/правки, но блокируют
запросы к изменённому endpoint. Новое подключение создаёт новые вкладки.
Версия workspace/Cargo.lock/UI/bundle — 0.1.9, следующая — 0.1.10.

Добавлены SQLite worker-регрессии: повреждённое нераскрытое view не мешает Connect
и списку таблиц; отказ Describe не ломает другую таблицу; profile Update сохраняет
порядок/другие профили, разрывает старый pool и отвергает duplicate/stale/untested
credential updates. Pointer-driven egui-тест редактирует имя и SQLite-путь через
форму, сохраняет профиль, проверяет dirty-вкладку прежнего endpoint и открытие
данных нового, ошибку дубликата и Cancel. Пользовательские подключения/БД не
используются в автоматических тестах.

Проверки v0.1.9: полный native workspace/all-features test с system access для
существующих localhost TCP mocks — 225 passed, 0 failed, 5 ignored. Первый
sandbox-прогон затронутых крейтов блокировал четыре прежних TCP mock-теста с
PermissionDenied; новые тесты БД и формы прошли. Native Clippy workspace/all-targets/
all-features с -D warnings, rustfmt, workspace release build и git diff --check
прошли. PostgreSQL/MySQL/MariaDB live tests и Windows build/Clippy не запускались.

Нативная macOS-проверка упакованной 0.1.9 использовала отдельную временную app-копию
с уникальным bundle ID, временными settings/project и SQLite-файлом с broken view.
Connect показал Online/Database Connected и дерево; раскрытие broken view показало
локальную ошибку с Retry, после неё исправная таблица открылась с данными.
Edit Connection… открыл предзаполненную форму с доступными Save/Cancel. Сохранение
изменённых полей и защиту старых вкладок проверяет pointer-driven egui regression;
нативное сохранение изменённых полей не подтверждено. Тестовое окно закрыто.
Пользовательские профили, credentials и реальные БД не изменялись. macOS ARM64
executable и подписанный app archive обновлены update-binaries.py; --check проверяет
source fingerprint и SHA-256 payload. Windows-кэш не создавался, commit/push нет.


### Source-Available лицензия и legal materials (v0.1.10, 2026-10-31)

Apache-2.0 для собственного кода заменена Nozhik Source-Available License 1.0,
правообладатель Sergei Peshalov. Разрешены аудит, обучение, изменения, исходные
форки/PR и бесплатная самостоятельная сборка для собственного использования,
включая коммерческую работу и внутреннее использование организацией. Передача
бинарников другим пользователям за пределами разрешённого внутреннего
использования, включая бесплатные/изменённые/переименованные сборки, требует
письменного разрешения. Купленная официальная сборка лицензируется бессрочно
на пользователя, без лимита устройств, обязательной активации или подписки;
будущие обновления/поддержка определяются условиями покупки. Пользовательские
проекты и результаты не подпадают под лицензию IDE. Сторонние права сохранены.
CONTRIBUTING.md объясняет неисключительное право использовать и перелицензировать
вклады без передачи авторства. Git history не переписывалась.

Все 14 Cargo packages наследуют license-file и версию 0.1.10; lockfile обновлён.
README, AGENTS, architecture и coding standards согласованы. THIRD_PARTY_NOTICES.md
описывает компоненты, scope target-specific inventory и отдельную проверку
Windows GNU runtime/новых ресурсов. scripts/third_party_licenses.py собирает
license/notice/authors тексты, включая nested libgit2/OpenSSL и fonts; MPL source
и registry packages без отдельных license files поставляются в ZIP с сохранением
исходных материалов. Для отсутствующих отдельных текстов используется объявленный
permissive вариант из licenses/. Collector не является проверкой совместимости.

update-binaries.py включает LICENSE, THIRD_PARTY_NOTICES.md, THIRD_PARTY_LICENSES.txt
и THIRD_PARTY_SOURCES.zip рядом с executable и в Resources/Legal подписанного
macOS bundle. Legal inputs входят в source fingerprint и CI paths; check требует
наличия legal sidecars в payload metadata. Изначальная Apache-лицензия сохранена
в licenses/Apache-2.0.txt только для сторонних компонентов.

Проверки: native workspace/all-features tests — 225 passed, 0 failed, 5 ignored
после предоставления system access для существующих localhost/process mocks.
Первый sandbox-прогон блокировал шесть AI mock/lifecycle tests с PermissionDenied.
Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt, workspace
release build и git diff --check прошли. Проверены наследование root LICENSE
в Cargo metadata, MPL sources в архиве, libgit2 linking exception и font notices,
изменение fingerprint при изменении LICENSE. macOS ARM64 executable и app archive
обновлены штатным скриптом, --check подтвердил источники и payload hashes.
ZIP integrity, версия 0.1.10 в plist, совпадение четырёх legal files с sidecars
и codesign --verify --deep --strict распакованного bundle подтверждены.
UI-логика не менялась; native GUI, Windows build/Clippy и live DB tests не
запускались. Windows-кэш не создавался; commit/push не выполнялись.

### Игнорирование директорий из Git Changes (v0.1.10, 2026-11-02)

Сразу под игнором выбранных файлов добавлен `Ignore Folder (Add to .gitignore)`
в существующем стиле меню. Клик по названию игнорирует родительскую папку файла
под правым кликом (также при multiselect); правая стрелка открывает стандартное
submenu egui с предками от ближайшего к верхнему, исключая корень репозитория.
Для `.nozhik/logs/log.txt` доступны `.nozhik/logs` и `.nozhik`; для файла в корне
действие отключено. Mutation busy gate сохранён.

`Task::IgnoreFolder` работает на serial Git worker, использует общую с Ignore
валидацию и экранирование, добавляя anchored directory-only pattern `/folder/`
в корневой `.gitignore`. Tracked entries не удаляются из index. Добавлены тест
worker для вложенности/anchoring/спецсимволов/недопустимых путей/сохранения tracked
entries и headless egui-регрессия с реальными pointer events для прямого клика,
открытия стрелкой без записи и выбора каждого из двух предков.
Workspace/Cargo.lock/UI/bundle — 0.1.10, следующая версия — 0.1.11.

Проверки: полный native workspace/all-features test — 227 passed, 0 failed,
5 existing system/live tests ignored. Первый sandbox-прогон блокировал
существующие localhost TCP mocks с PermissionDenied; повторный полный прогон
с system access прошёл. Native Clippy workspace/all-targets/all-features
с -D warnings, rustfmt, workspace release build и git diff --check прошли.
macOS ARM64 executable и подписанный app archive обновлены update-binaries.py;
--check подтвердил source fingerprint и SHA-256 payload. UI проверен headless
через pointer events; нативное GUI-окно в этой итерации не запускалось.
Windows build/Clippy/GUI, commit и push не выполнялись.

### AI-заполнение сообщения коммита (v0.1.11, 2026-11-02)

Над Summary/Description добавлена `Auto-fill Commit Message`, доступная при
включённом AI completion, выбранной модели и хотя бы одном staged-файле.
Кнопка использует ту же локальную Ollama/ai_model, работает асинхронно и показывает
Generating/Cancel. Модель возвращает редактируемые английские summary/description;
коммит создаётся только прежней кнопкой. Ошибка сохраняет прежний текст формы.

Новый read-only Git task CommitContext передаёт ограниченный HEAD→index patch,
без unstaged/unsaved/untracked contents, с binary metadata и truncation marker
при превышении 24 KiB. Index перечитывается с диска. Fingerprint HEAD/ref и всех
index entries проверяется повторно после HTTP-ответа. Ручное изменение формы,
Cancel, изменение AI enabled/model, проекта, Git mutation и shutdown отменяют
запрос; stale replies не перезаписывают поля. Модели не скачиваются автоматически.

Ollama transport использует schema summary/description и отдельные от FIM options:
512 tokens, context 16384, deadline 120s, bounded response 64 KiB. Некорректные поля,
незавершённая/оборванная генерация отклоняются. Тест worker проверяет staged-only,
unborn HEAD, отсутствие index writes, большие Unicode patch и изменение identity.
HTTP mock проверяет schema/model/options, multiline description и invalid output.
Pointer-driven egui проверяет положение кнопки, disabled states, заполнение формы,
ошибку HTTP, Cancel/disable с закрытием соединения, ручные правки, внешнее изменение
index и отсутствие автоматического коммита. Добавлен ignored live Ollama smoke
на синтетическом diff. Workspace/Cargo.lock/UI/bundle — 0.1.11, следующая — 0.1.12.

Верификация v0.1.11: полный native workspace/all-features test с system access
для локальных TCP mocks — 230 passed, 0 failed, 6 ignored (включая новый live smoke).
Live smoke отдельно выполнен на установленной локальной qwen2.5-coder:1.5b-base
с искусственным diff замены вычитания на сложение: получены summary/description.
Пользовательские изменения в live-запрос не отправлялись. После сокращения
Generating… для узкой панели pointer-driven AI regression повторно прошла.
Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt,
workspace release build и git diff --check прошли. Нативное GUI-окно не запускалось;
Windows build/Clippy/GUI, commit и push не выполнялись.
macOS ARM64 executable и подписанный Nozhik.app.zip обновлены через
update-binaries.py после всех правок; --check подтвердил source fingerprint
и SHA-256 payload. Windows-кэш не создавался.

### Настройки Git проекта и общая аутентификация (v0.1.12, 2026-11-02)

Settings → Git доступен для любого открытого репозитория, включая проект,
клонированный сторонним клиентом и открытый как папка. Форма показывает repo root,
origin fetch URL, отдельный optional push URL, commit author name/email,
authentication (system/helper/agent, HTTPS token, login/password, SSH key/passphrase)
и credential helper. Save/Reload/Test работают через serial Git worker; общий
AuthFields используется также в Clone Repository. Обычный Config auto-save не меняет
Git config — для него есть явная Save Git Settings.

Новые GitSettings/AuthProfile/AuthMethod содержат только несекретные поля.
SaveGitSettings проверяет входные поля, protocol/auth compatibility и stale snapshot,
записывает common-dir config атомарно под стандартным config.lock с сохранением
прочих ключей/permissions. Секреты не сериализуются. Пустые identity/helper overrides
используют системные настройки; прежний push URL при пустом поле заменяется fetch URL.
Origin changes не меняют refs/index/worktree и не запускают сеть. После смены origin
нужно явно сделать Fetch для актуализации remote-tracking refs.

Fetch/Pull/Push теперь используют repository-local Git credential config и общую
с clone credential resolution, с session Authentication для выбранных endpoint/profile.
Внешнее изменение URL/method прекращает использование старого секрета; callbacks
не передают credentials при redirect и ограничивают число попыток. TLS/SSH trust
проверки сохранены. Sanitized transport errors указывают Settings → Git; ошибки
commit identity отдельно указывают поля автора. Test Fetch Connection не сохраняет
черновик/refs и не проверяет push permissions; изменённый helper сначала сохраняется.

Clone сохраняет только nozhik.auth.method/username/keypath, затем передаёт
Credentials { url, authentication } с путём в workspace prepared-open request
целевого проекта. Authenticate ставится после Open и повторно проверяет исходный
URL, отсутствие нового push URL и соответствие profile. Это исключает применение
clone token после внешней смены remote. Dialog secrets очищаются после передачи;
проект хранит их только в zeroizing session memory до закрытия. После повторного
открытия токен/пароль требуется заново, либо используется helper/SSH agent.
При отмене/ошибке открытия credentials отбрасываются; глобального credential cache нет.

Новые backend-регрессии проверяют local helper, author identity/local commit,
origin/push URL, отсутствие secret на диске, сохранение matching session credentials,
повторное открытие без секретов, stale/invalid/config.lock gates, read-only connection
test и отказ clone credentials при смене URL до Open. Pointer-driven UI проверяет
навигацию Settings → Git, редактирование полей и методов, Save/Reload/Test и отсутствие
записи токена. Дополнительные регрессии проверяют transfer/очистку clone dialog и
порядок Open→Authenticate. Workspace/Cargo.lock/UI/bundle — 0.1.12; следующая — 0.1.13.

Проверки v0.1.12: полный native workspace/all-features test с system access для
локальных TCP mocks — 238 passed, 0 failed, 6 existing optional system/live tests
ignored. Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt,
workspace release build и git diff --check прошли. macOS ARM64 executable и
подписанный Nozhik.app.zip обновлены update-binaries.py; --check подтвердил source
fingerprint и SHA-256 payload. Windows-кэш не создавался. Нативное GUI-окно и
реальная авторизация на GitHub/SSH не проверялись; UI проверен headless pointer
regressions, transport/helpers — на временных локальных fixtures. Реальные remotes,
user identity, credentials и настройки пользователя не менялись. Windows build/Clippy,
commit и push не выполнялись.

### GitHub token access diagnostics (v0.1.13, 2026-11-02)

По явному разрешению пользователя выполнена read-only проверка переданного PAT:
GitHub /user подтвердил его валидность (200), repository metadata/contents вернули
404, Git upload-pack advertisement вернул 403 с отказом доступа. Токен не записывался
в файлы, Git config, process arguments или вывод; вводился через stdin с отключённым
echo. Сразу после диагностики пользователю сообщено о завершении и необходимости
отозвать раскрытый token. Никаких commits/push/config writes на реальном проекте
не было. Результат указывает на доступ token к репозиторию/permissions, а не TLS.

Общая ошибка v0.1.12 скрывала HTTP status. Новый transport_error формирует фиксированные
сообщения для 401/auth, 403 (GitHub Resource owner/repo/Contents/approval), 404/private
repo, TLS/SSH, DNS/network, proxy, rate limits, server errors и non-fast-forward.
Raw error body/URL/credentials по-прежнему не выводятся. Пустые session secrets
обнаруживаются до сети; token whitespace trimmed, USERNAME использует x-access-token.
Test удерживает RAII connection до чтения advertised refs, без двойного disconnect.

Добавлены проверка safe diagnostics с фиктивным server-echo secret, HTTP mock
401→Basic auth→403 с фиктивным токеном и ignored live diagnostic с URL/token на stdin.
Пользовательские token/URL не зашиты в тестах. Workspace/Cargo.lock/UI/bundle — 0.1.13,
следующая версия — 0.1.14.

Верификация v0.1.13: полный native workspace/all-features test с system access
для localhost mocks — 240 passed, 0 failed, 7 ignored. Новый live GitHub
diagnostic не входит в обычный прогон; разрешённая live-проверка выше воспроизвела
403, после неё реальный токен больше не использовался. Новый unit-тест выявил
обработку HTTP code с пунктуацией после числа; parser исправлен, финальный полный
прогон прошёл. Native Clippy workspace/all-targets/all-features с -D warnings,
rustfmt, workspace release build и git diff --check прошли. macOS ARM64 executable
и подписанный app archive обновлены update-binaries.py; --check подтвердил source
fingerprint и SHA-256 payload. Windows build/Clippy/GUI, native GUI, commit/push
не выполнялись; Windows-кэш не создавался.


### Слияние Git-доработок и смены лицензии (v0.1.14, 2026-11-02)

Разрешены конфликты master/imp_0 в версиях и документации. Сохранены Source-Available
LICENSE, CONTRIBUTING, THIRD_PARTY_NOTICES, наследование license-file во всех Cargo
packages, legal collector и упаковка notices/sources в локальные дистрибутивы.
Сохранены Git folder ignore, Ollama commit message generation, project Git settings,
clone/session authentication и безопасная transport diagnostics ветки imp_0.
Документация обеих веток объединена без удаления исторических записей. Версия
workspace/Cargo.lock/UI/bundle — 0.1.14, следующая доработка — 0.1.15.

Проверки merge: полный native workspace/all-features test с system access и
--test-threads=2 — 240 passed, 0 failed, 7 ignored. Первый запуск сборок
остановился из-за нехватки диска; после удаления incremental cache сборка прошла.
Параллельный test/release прогон затем дал timeout существующего файлового UI-теста;
повторный полный прогон без параллельной сборки и с двумя test threads прошёл.
Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt, workspace
release build и git diff --check прошли. Все 14 Cargo packages через metadata
проверены на 0.1.14 и наследование root LICENSE. Rust sources совпадают с imp_0,
лицензионные файлы/collector/упаковка сохранены из master. macOS ARM64 executable
и подписанный app archive обновлены update-binaries.py; --check подтвердил source
fingerprint и payload hashes. В ZIP проверены версия bundle, четыре legal files
с точным совпадением sidecars и целостность THIRD_PARTY_SOURCES.zip (1846 entries).
Native GUI, Windows build/Clippy и реальные GitHub credentials не использовались.

По отдельному запросу пользователя удалён весь генерируемый target через cargo clean
(199913 files, 68.1 GiB logical), локальный Windows-комплект (53 MiB) и 16 untracked
Finder .DS_Store. Измеренный allocated size проекта: 44.34 GiB →
377.0 MiB, освобождено 43.97 GiB allocated storage.
Исходники, .git/history, конфигурация и готовый macOS-комплект с legal materials
сохранены. Windows source/build support сохранена; Windows-кэш не создавался.
Проверки завершены до очистки; новая компиляция снова создаст target.


### Визуализация объектов БД по референсам (v0.1.15, 2026-11-02)

Database Explorer получил компактные строки, точные цвета и размеры по изображениям:
24 px высота, 19 px вложенность, основной/вторичный текст 14/12 px. Добавлены
оригинальные векторные иконки namespaces, папок, таблиц/views, колонок с признаками
nullable/PK/FK, жёлтых ключей, синих внешних ключей и индексов с U для UNIQUE.
Выделение занимает всю ширину строки, счётчики и типы отделены от названия.
Убраны кнопки между уровнями дерева: управление подключением доступно по правому
клику, offline Connect также остаётся внутри ветки. Единственный namespace с именем
текущей database не дублируется. Double-click/open, lazy Describe/retry и SQL consoles
сохранены. SchemaColumn получает FK/default из каталогов, MySQL object annotations
компактно показывают columns/reference/UNIQUE. Версия 0.1.15, следующая — 0.1.16.

Проверки v0.1.15: полный native workspace/all-features test с system access для
существующих TCP/process mocks и двумя test threads — 242 passed, 0 failed,
7 optional system/live tests ignored. Native Clippy workspace/all-targets/all-features
с -D warnings, rustfmt и git diff --check прошли. Pointer-driven explorer/table
и connection-edit regressions сохранены; новая проверка контролирует высоту,
вложенность, выделение и цвет аннотации выбранной строки. SQLite-тест проверяет
PK/FK/nullable/default. Для отсутствующей в UI font стрелки связи добавлен
векторный рендер и отдельный regression без зависимости от font glyph.

Нативная macOS-проверка выполнялась отдельной временной app-оболочкой с отдельным
bundle ID, временными settings/project и синтетической SQLite-базой: подключение,
раскрытие схемы/tables/table и всех четырёх групп, иконки и выделение подтверждены.
Выявленный missing-glyph square для → устранён после этого осмотра векторной
стрелкой; исправление проверено headless regression. Пользовательские настройки,
подключения и реальные БД не изменялись. PostgreSQL/MySQL live tests и Windows
build/Clippy не запускались. Commit/push не выполнялись.

Финальный workspace release build прошёл. macOS ARM64 executable и подписанный
Nozhik.app.zip обновлены штатным update-binaries.py; --check подтвердил source
fingerprint и payload hashes. В ZIP проверены версия 0.1.15, целостность архива
и совпадение всех четырёх legal sidecars с Resources/Legal. После проверки сборок
выполнен cargo clean (3.1 GiB), удалены временная тестовая среда и 5 untracked
Finder files. Размер проекта после очистки — 377 MiB; готовый macOS-комплект
с legal materials сохранён. Windows-кэш не создавался.


### GEOMETRY и серверная пагинация таблиц (v0.1.16, 2026-11-02)

Исправлено открытие goods с nullable POINT/GEOMETRY: MySQL decoder читает
пространственный payload как байты, а не String. DbValue::Geometry сохраняет
SRID+WKB; POINT показывается читаемо, другие формы — hex. Колонка GEOMETRY
доступна для чтения. Submit обычных колонок сравнивает исходные geometry bytes
через CAST AS BINARY, не меняет координаты и не теряет SRID. Geometry включена
в общий retained byte limit. Новых зависимостей нет.

Таблицы открываются первыми 500 строками. Серверный SELECT запрашивает только
page_size+1 с OFFSET и ORDER BY PK при наличии ключа; lookahead не попадает
в редактируемый snapshot. Нижняя панель под viewport показывает диапазон,
нижнюю оценку количества строк (501+) или окончание, First/Previous/Next и выбор
100/500/1000/5000 строк. Номера строк учитывают offset. Переходы заблокированы
при busy/modal/dirty, ошибка запроса сохраняет прежнюю страницу. Reload и Submit
сохраняют текущую страницу. Полный COUNT не выполняется; внешний concurrent DML
может сдвинуть OFFSET, без PK порядок зависит от сервера.

Добавлены SQLite regression на 1201 строку и quoting идентификаторов, geometry
SRID/WKB/literal/predicate regression для обоих byte orders, pointer-driven UI
regression первой/средней/последней страницы, положения footer, смены размера,
dirty gate и ошибки загрузки с сохранением snapshot. Версия workspace/Cargo.lock,
UI и bundle — 0.1.16, следующая — 0.1.17.

Верификация v0.1.16: полный native workspace/all-features test с system access
для существующих localhost/process mocks и двумя test threads — 245 passed,
0 failed, 7 optional system/live tests ignored. Первый sandbox-прогон блокировал
TCP mocks; финальный полный прогон с разрешённым system access прошёл.
Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt,
workspace release build и git diff --check прошли. macOS ARM64 executable и
подписанный app archive обновлены штатным update-binaries.py; --check подтвердил
source fingerprint и SHA-256 payload. В ZIP подтверждены версия 0.1.16,
целостность, совпадение четырёх legal sidecars и целостность sources archive.
Нативное GUI, live MySQL/PostgreSQL и пользовательская база не проверялись;
UI проверен headless pointer regression. Локальный MySQL/Docker service недоступен.
Windows build/Clippy, commit и push не выполнялись; Windows-кэш не создавался.

После проверки готовых бинарников выполнен cargo clean (4.5 GiB), удалены пять
untracked Finder files и временные логи агента. Итоговый размер проекта — 378 MiB;
исходники, .git, настройки и актуальный macOS-комплект с legal materials сохранены.


### Чтение временных типов MySQL (v0.1.17, 2026-11-02)

Исправлена следующая ошибка открытия goods: NaiveDateTime в SQLx не совместим
с native TIMESTAMP. MySQL DATE/TIME/DATETIME/TIMESTAMP теперь читаются из
COM_QUERY text results как точное серверное представление в DbValue::DateTime.
Сохраняются clock values сессии без добавления UTC/offset, микросекунды,
legacy zero dates и отрицательные/многосуточные TIME. SQL NULL остаётся Null.
PostgreSQL/SQLite и страницы по 500 строк сохраняют прежнюю логику.

Добавлен bounded localhost MySQL wire fixture: настоящий SQLx получает
TIMESTAMP/DATETIME/DATE/TIME native field metadata, а Connection::query проходит
полный production decode. Проверяются обычные и дробные значения, NULL,
нулевые даты и TIME -838:59:59.000001. Новых зависимостей нет.
Версия workspace/Cargo.lock/UI/bundle — 0.1.17; следующая — 0.1.18.

Верификация v0.1.17: новый wire regression сначала запущен с прежним decoder
и воспроизвёл точную ошибку NaiveDateTime/DATETIME vs TIMESTAMP. После восстановления
исправления полный native workspace/all-features test с system access для
localhost/process mocks и двумя test threads прошёл: 246 passed, 0 failed,
7 existing optional system/live tests ignored. Native Clippy workspace/all-targets/
all-features с -D warnings, rustfmt и workspace release build прошли.
Все 14 workspace packages в Cargo.lock используют 0.1.17. Нативное GUI,
реальные MySQL/PostgreSQL и пользовательская БД не использовались; wire fixture
изолирован на localhost и не требует установленного сервера или credentials.
Windows build/Clippy, commit и push не выполнялись.

macOS ARM64 executable и подписанный app archive обновлены update-binaries.py;
--check подтвердил source fingerprint и payload hashes. ZIP проверен на
целостность, bundle version 0.1.17 и совпадение четырёх legal sidecars;
THIRD_PARTY_SOURCES.zip также целостен. После проверки бинарников выполнен
cargo clean (5.0 GiB), удалены пять untracked Finder files и временные файлы
агента. Размер проекта — 378 MiB. Готовый macOS-комплект сохранён; Windows-кэш
не создавался. Финальный git diff --check прошёл.


### WHERE, ORDER BY и стрелки сортировки (v0.1.18, 2026-11-02)

Над редактируемой таблицей добавлены WHERE / ORDER BY с векторными иконками,
крестиками очистки и Apply; Enter применяет оба поля. Сервер получает фильтр и
порядок до LIMIT/OFFSET, применение сбрасывает страницу на первую. Выборка
сохраняется при смене размера/страницы, Reload и Submit. Заголовки получили
векторный цикл нейтральных up/down triangles → ASC/up → DESC/down → neutral;
стрелка меняет ORDER BY одной колонки. По умолчанию нет скрытой PK-сортировки.
Простые ручные ORDER BY синхронизируют иконки, сложные выражения остаются neutral.

TableQuery проверяет границы SQL fragments, quotes/скобки, запрещает statements/
comments и внешние clauses; quoted punctuation допускается. Типовые и прочие
синтаксические ошибки показываются от сервера. Черновики полей, применённые и
pending условия разделены: ошибка/отмена сохраняет прежний snapshot и условия.
Dirty/busy/modal/invalid gates защищают staged edits. Новых зависимостей нет.

Добавлены backend execution regression на 1201 строку (WHERE до paging,
глобальный DESC, multi-column ORDER BY, пустая выборка, отсутствие default ORDER,
quoting и отказ script/clause escapes) и pointer-driven UI regression цикла
стрелок, Enter/Apply, очистки, paging, dirty gate, Submit и ошибки SQL.
Версия workspace/Cargo.lock/UI/bundle — 0.1.18; следующая — 0.1.19.

Верификация v0.1.18: финальный полный native workspace/all-features test с
system access для localhost/process mocks и двумя test threads — 248 passed,
0 failed, 7 existing optional system/live tests ignored. Native Clippy
workspace/all-targets/all-features с -D warnings, rustfmt, workspace release
build и git diff --check прошли. Все 14 workspace packages — 0.1.18.
UI проверен headless pointer-driven: нейтральный/ASC/DESC цикл, глобальный
результат (DESC начинается с 1201, а не 500), WHERE, Enter, Apply, очистка ORDER,
переход на последнюю страницу выборки, dirty gate, Submit, пустая выборка и
server SQL error со старым snapshot. Backend дополнительно проверяет цитирование,
quoted punctuation и запрет выхода из выражений. Реальные MySQL/PostgreSQL,
пользовательские БД и нативное GUI не запускались. Windows build/Clippy,
commit и push не выполнялись; Windows-кэш не создавался.

macOS ARM64 executable и подписанный Nozhik.app.zip обновлены штатным
update-binaries.py; --check подтвердил source fingerprint и SHA-256 payload.
Проверены версия bundle 0.1.18, целостность обоих ZIP и совпадение четырёх legal
sidecars с Resources/Legal. После проверки бинарников выполнен cargo clean
(4.5 GiB), удалены четыре untracked Finder files и временные логи агента.
Размер проекта после очистки — 378 MiB. Исходники, .git, настройки, предыдущие
незакоммиченные доработки и готовый macOS-комплект сохранены.


### Условия платной лицензии и спецификация активации (v0.1.19, 2026-11-02)

Создана paid-software-licensing.md: будущая реализация после релизной версии,
первая привязка ключа к email без предварительного кода, дополнительные устройства
через почтовое подтверждение, смена email через старую и новую почту, отзыв
прежних активаций, семидневное подписанное офлайн-разрешение и редкое продление.
Зафиксированы модель данных, минимальные серверные операции, покупки на своей
и сторонних площадках, ограничения защиты, критерии приёмки и открытые настройки.
Сервер, лаунчер и клиентские проверки не реализованы.

LICENSE обновлён до Nozhik Source-Available License 1.1: бессрочная покупка на
пользователя, три активированных устройства для лицензированного продукта,
активация и привязка к email. До доступности механизма техническая активация
не требуется; установленная для использования копия занимает место, резервная
копия сама по себе — нет. Прежние покупки автоматически не переводятся на новые
условия. Права самостоятельной сборки и сторонних компонентов сохранены.
README и architecture синхронизированы; workspace/Cargo.lock/UI/bundle — 0.1.19,
следующая доработка — 0.1.20. Код поведения приложения не менялся.

Проверки: native Clippy workspace/all-targets/all-features с -D warnings, rustfmt,
полный workspace/all-features test с разрешённым system access для локальных
TCP/process fixtures и двумя test threads — 248 passed, 0 failed, 7 ignored;
workspace release build прошёл. Версии всех 14 локальных пакетов и ссылки
документации проверены. macOS ARM64 комплект обновлён update-binaries.py;
--check подтвердил source fingerprint и payload hashes. GUI, Windows build/Clippy,
commit и push не выполнялись.

В ZIP проверены версия 0.1.19, целостность архивов и точное совпадение всех четырёх
legal sidecars с Resources/Legal; LICENSE в поставке совпадает с корневым.
После проверки выполнен cargo clean (6.5 GiB), удалены временные файлы задачи.
Размер проекта после очистки — 379 MiB; готовый macOS-комплект сохранён,
Windows-кэш не создавался. git diff --check прошёл.


### SQL completion и редактируемые результаты консоли (v0.1.20, 2026-11-02)

Добавлены schema completion для таблиц и полей в SQL-консолях, aliases и lazy
Describe через существующий worker. Штатный completion popup принимает Tab/Enter
и Ctrl+Space, не активируется внутри строк/комментариев. Результаты используют
TableEditor без WHERE/ORDER BY: редактирование, добавление/удаление, Revert, preview,
Submit и reload исходного запроса. Прямые проекции одной таблицы с полным PK
редактируемы; неоднозначные/вычисляемые результаты доступны для чтения.
WHERE type=11 предзаполняет новую строку, включая непоказанные в SELECT поля;
неравенства/OR/NOT/выражения не дают предполагаемых значений. Добавлены keyboard,
provenance/defaults, SQLite DML и pointer-driven worker/UI regression.
Workspace/Cargo.lock/UI/bundle — 0.1.20, следующая доработка — 0.1.21.

Проверки v0.1.20: полный native workspace/all-features test с system access для
localhost/process fixtures и двумя test threads — 254 passed, 0 failed, 7 ignored.
Native Clippy workspace/all-targets/all-features с -D warnings и rustfmt прошли.
Keyboard regression проверяет таблицу/поле, Tab/Enter и подавление в строках;
SQLite regression — DML с constraint непоказанного поля; pointer-driven worker/UI
regression — 601 строку без обрезания до 500, preview, dirty Execute gate,
добавление/удаление и Submit/reload выполненного SQL. Локальная сортировка
сохраняет row identity и defaults, OR/NOT/неравенства/конфликты/дубликаты
равенств покрыты отдельно. Реальные MySQL/PostgreSQL и нативное GUI не запускались.
Windows build/Clippy, commit и push не выполнялись.

Workspace release build и штатный update-binaries.py прошли. macOS ARM64 executable
и подписанный Nozhik.app.zip обновлены; --check подтвердил source fingerprint и
payload hashes. Проверены bundle version 0.1.20, целостность обоих ZIP и совпадение
четырёх legal sidecars с Resources/Legal. После проверки бинарников выполнен
cargo clean (9.8 GiB logical), удалены 6 untracked Finder files и временные логи.
Размер проекта после очистки — 379 MiB; готовый macOS-комплект и legal materials
сохранены. Windows-кэш не создавался. Финальный git diff --check прошёл.


### Контекстное меню схемы и встроенный экспорт (v0.1.21, 2026-11-02)

Каждая схема/БД теперь имеет отдельный узел и меню: New SQL console сразу выбирает
namespace; Modify Schema показывает реальные настройки и точный SQL diff;
Generate DDL to Clipboard копирует определения объектов; Run SQL Script выбирает
UTF-8 файл штатным picker и выполняет все команды; Export Data to File предлагает
SQL всей схемы либо CSV выбранной таблицы, параметры и bounded preview.
Для MySQL/MariaDB редактируются collation/schema grants, PostgreSQL — имя, owner,
comment/USAGE/CREATE, SQLite — user_version/application_id. Stale settings перед
Apply отклоняются; pending requests защищены ID/cancellation и modal gates.

Экспорт полностью встроенный, через существующие SQLx pools и SSH transports;
pg_dump/mariadb-dump/mysqldump и другие установленные программы не нужны.
MySQL/MariaDB использует SHOW CREATE, typed server literals, routines/triggers/events,
view placeholders, BIT/blob hex, WKB+SRID, sequence high-water marks. PostgreSQL
использует catalog deparse, repeatable-read snapshot и server quote_nullable:
tables/partitions/inheritance, identity/sequences, enum/domain/composite types,
functions/procedures, views/mviews, keys/indexes, triggers/rules/RLS policies,
collations/statistics, owners/comments/ACL. SQLite сохраняет ordinary rowids,
generated exclusions, sqlite_master и AUTOINCREMENT state. Generated columns
восстанавливаются сервером; triggers следуют данным. Неподдерживаемые специальные
PostgreSQL-категории и SQL data SQLite virtual tables дают явную ошибку,
CSV видимых строк virtual tables остаётся доступен. Список ограничений в README.

Artifact владеет полным временным файлом; preview ≤64 KiB, clipboard ≤32 MiB,
transpose ≤16 MiB. Обычный SQL/CSV экспорт потоковый, без лимита строк консоли.
Смена настроек сбрасывает старый preview; Export/Copy используют ту же генерацию.
Финальная запись атомарна и не портит destination при ошибке генерации.
Run SQL Script drains результаты без retained row cap, обрабатывает MySQL
DELIMITER вне строк/комментариев и SQL_MODE NO_BACKSLASH_ESCAPES. SQLite cancel/
незавершённый BEGIN не оставляют transaction/progress-handler в следующей операции.
Скрипты ограничены 64 MiB; committed DDL/DML при ошибке/отмене могут сохраниться.
Новых зависимостей нет. Все 14 workspace packages/Cargo.lock/UI/bundle — 0.1.21,
следующая доработка — 0.1.22.

Проверки: полный native workspace/all-features test с system access для
localhost/process fixtures и двумя test threads — 258 passed, 0 failed, 8 ignored.
Новый optional live regression отдельно успешно выполнен на изолированных
PostgreSQL 14.4 и MariaDB из существующих Docker images: dump/drop/restore,
10 005/16 384 строк, schema/identifier quoting, FK ordering, view dependencies,
functions/procedures/events/triggers, generated/identity, sequence state,
DECIMAL/bytea/BIT/geometry/SRID/arrays/time, comments/ACL и изменение settings.
Контейнеры задачи удалены вместе с tmpfs-данными, пользовательские БД не трогались.
SQLite регрессии покрывают >10 000 строк, round-trip, CSV escaping/headers/NULL/
transpose, атомарное сохранение, stale diff, script errors/cancellation/rollback.
Headless pointer-driven UI regression проверяет меню, выбранную схему консоли,
SQL preview/Apply, экспорт и invalidation, clipboard. Native GUI вручную не запускался.
Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt,
workspace release build прошли. Windows build/Clippy, commit и push не выполнялись.

Штатный update-binaries.py обновил macOS ARM64 executable и подписанный app archive;
--check подтвердил source fingerprint и SHA-256 payload. Проверены bundle version
0.1.21, целостность обоих ZIP и совпадение четырёх legal sidecars с Resources/Legal.
После проверки выполнен cargo clean (11.4 GiB), удалены пять untracked Finder files
и временные файлы/логи задачи. Размер проекта после очистки — 381 MiB;
актуальный macOS-комплект сохранён, Windows-кэш не создавался.


### Редактор таблиц, DDL/экспорт и загрузка вкладки (v0.1.22, 2026-11-03)

Меню таблицы содержит Modify Table, Generate DDL to Clipboard и Export Data to File.
Модальное окно следует референсам: дерево объектов слева, редактируемые свойства
справа, подсвеченный SQL-preview снизу. Изменения остаются черновиком до Apply/OK.
Поддержаны table properties, columns/default/generated/identity/sequence options,
primary/unique/foreign/check/exclusion keys, самостоятельные indexes, triggers,
комментарии и явные grants с учётом диалекта. Новые IDs монотонны; удаление и
добавление одноимённого объекта не смешивает его с исходным. Constraint-backed
indexes представлены ключами. Несохранённые данные и активная загрузка блокируют
структурное редактирование. Stale catalog отклоняется до Apply; Reload перечитывает
структуру, включая частично применённый MySQL DDL после ошибки.

Один migration planner формирует preview и исполняемый SQL. PostgreSQL использует
транзакцию и native ALTER, MySQL/MariaDB — объединённый ALTER без неявного усечения
значений и с сохранением trigger DEFINER/creation SQL_MODE (включая пустой mode).
SQLite native rename отделён от транзакционной перестройки; сохраняются обычные
rowids, AUTOINCREMENT high-water mark, данные и неизменённые native clauses.
Перед commit проверяются FK и dependent views; cancel/error восстанавливает session
pragmas/progress handler. Автоматического CASCADE нет. Ограничения virtual tables,
partition-layout и порядка существующих PostgreSQL columns описаны в README.

Table-only SQL/CSV использует существующий встроенный exporter и штатный picker,
без внешних программ. Catalog filters исключают чужие tables/routines/events;
PostgreSQL сохраняет same-schema descendants, owned sequences, функции триггеров,
комментарии ключей/индексов/триггеров и ACL. Внешние зависимости остаются внешними.
Whole-schema dump сохраняет прежнюю область. Generate DDL использует тот же exporter.

Двойной клик немедленно создаёт вкладку с metadata-loader, затем data-loader.
BufferId связывает ответ с вкладкой; закрытие до ответа и переключение вкладок
не приводят к повторному открытию или смене фокуса. Ошибки/отключение снимают
loader; Retry повторяет metadata request. Refresh catalog перезапускает актуальные
pending describes. Успешный Apply обновляет дерево и чистые вкладки таблицы.

Проверки: native workspace/all-features — 262 passed, 0 failed, 9 ignored.
Два optional live regression отдельно прошли на временных PostgreSQL 14.4 и MariaDB:
изменение существующих keys/FK/indexes/identity, storage/collation, deferred constraint
trigger, пустой trigger SQL_MODE, table/schema dump/drop/restore и scope isolation.
SQLite проверки включают 100 000 строк, cancellation/rollback, зависимости и counters.
Pointer-driven headless UI проверяет реальные меню, изменение формы/Apply/export,
ранний loader, закрытие pending tab, сохранение фокуса и размеры окна 700 px.
Native GUI вручную не запускался. Clippy workspace/all-targets/all-features с
-D warnings и rustfmt прошли. Новых зависимостей нет. Версия всех workspace packages
и Cargo.lock — 0.1.22, следующая доработка — 0.1.23. Commit/push не выполнялись.

Workspace release build и штатный update-binaries.py завершены; macOS ARM64
executable и подписанный app archive обновлены. --check подтвердил source fingerprint
и SHA-256. Проверены bundle version 0.1.22, целостность ZIP и четыре legal sidecars.
Windows build/Clippy не запускались. Два временных контейнера удалены вместе с tmpfs;
пользовательские БД и настройки не менялись. Cargo clean удалил 11.9 GiB;
удалены временные логи и пять untracked .DS_Store. Итоговый размер проекта — 383 MiB,
готовый macOS-комплект сохранён. Финальные git status и diff --check проверены.

### CSS: подсветка, форматирование, индекс и палитра (v0.1.23, 2026-11-03)

CSS зарегистрирован как реализованный язык с расширением .css без учёта регистра.
Tree-sitter CSS работает в syntax worker, без PHP LSP и offline PHP catalogs.
Подсветка различает селекторы, свойства, значения, функции, строки, числа и комментарии.
Локальный форматтер подключён к Reformat Code и встроенному css_default.json:
отступы, границы блоков/деклараций и запятые, весь файл либо выделение. Проверяются
AST и точное сохранение токенов; malformed CSS отклоняется, CRLF сохраняется.

В gutter видимых строк рисуются цвета, включая stops градиентов. Поддержаны Hex,
named, RGB/RGBA, HSL/HSLA и другие литералы csscolorparser. Клик открывает графическую
палитру, RGBA и Hex с прозрачностью; изменение заменяет конкретный литерал через
EditTransaction с Undo. Цель привязана к BufferId/revision; внешние правки, Undo,
смена документа и modal gates закрывают окно. Exact RGBA хранится отдельно от
premultiplied Color32, чтобы Hex с alpha не округлялся. Однозначные document-local
var chains разрешаются до литерала: меняется определение, ссылки сохраняются.
Циклы, неоднозначные/внешние определения и вычисляемые цвета не угадываются;
каскад DOM и @import не вычисляются. Ширина gutter сохраняется до нового snapshot.

ProjectIndexer использует независимые PHP/CSS partitions с отдельными кэшами.
CSS хранит классы с учётом регистра, decoded escapes, путями и диапазонами;
комментарии, строки и имена псевдоклассов исключены. Unsaved overlays остаются
в памяти, Close восстанавливает disk facts; watcher/removal/invalidation обновляют
нужные файлы. Cache directory удаляется один раз перед rebuild всех partitions.
Автодополнение HTML оставлено отдельной задаче. Языковой opt-in сохраняется:
в старых проектах CSS включается через Settings → Languages, новые defaults — PHP/CSS.

Новые зависимости: tree-sitter-css (MIT), csscolorparser (MIT OR Apache-2.0),
transitive phf; legal materials включаются штатным packaging script. Все 14 workspace
packages, Cargo.lock, UI и bundle — 0.1.23; следующая доработка — 0.1.24.

Проверки: native workspace/all-features — 266 passed, 0 failed, 9 ignored. Один
SQL-console pointer test в первом параллельном прогоне не дождался UI label;
отдельная перепроверка и полный последовательный прогон прошли. Дополнительные CSS
проверки последней правки var chains также прошли. Headless pointer-driven regression
проверяет swatch → popup → Hex с alpha → точную замену → Undo и stale target gate.
Проверены formatter idempotence/selection, gradient/var cycles/ambiguity,
case-sensitive classes/escapes, cache reload, overlay/Close/deletion и CSS worker
без PHP server. Native Clippy workspace/all-targets/all-features с -D warnings,
rustfmt, workspace release build и git diff --check прошли. Native GUI вручную,
Windows build/Clippy, commit и push не выполнялись.

macOS ARM64 executable и подписанный Nozhik.app.zip обновлены штатным
update-binaries.py; --check подтвердил source fingerprint и SHA-256 payload.
Проверены bundle version 0.1.23, целостность обоих ZIP, четыре legal sidecars
и notices новых CSS-зависимостей. После проверки выполнен cargo clean (10.5 GiB),
удалены временные логи/скрипты задачи и шесть untracked Finder files. Размер проекта
после очистки — 383 MiB; актуальный macOS-комплект и legal materials сохранены.
Windows-кэш не создавался. Финальные git status и diff --check проверены.

### CSS completion и переходы между CSS/PHP (v0.1.24, 2026-11-03)

В CSS при вводе имени свойства работают встроенные стандартные подсказки через
прежний completion popup: дефисы, Tab/Enter, Ctrl+Space и сохранение существующего
двоеточия. Лексический контекст исключает значения, комментарии, строки, селекторы,
custom properties и контейнеры at-rules. В class-атрибутах PHP/PHTML и CSS-селекторах
предлагаются классы из индекса; замена ограничена текущим токеном и поддерживает Undo.

FileSymbols содержит общие ClassOccurrence и диапазоны статических class-атрибутов.
PHP extractor использует существующий AST, оставляя только markup nodes и literal
whitespace после закрывающего тега PHP. PHP-код/строки, HTML comments и raw тела
script/style/textarea/title исключены. Статические токены рядом с PHP-вставками
сохраняются только при разделении ASCII whitespace; динамические и entity-encoded
имена не угадываются. CSS-классы сохраняют регистр и decoded escapes. Другие языковые
extractors смогут передавать такие же факты; HTML/JS пока не включены.

ProjectIndexer публикует общий ClassIndex из retained facts PHP/CSS и объединённые
overlay revision gates. Cache schema 5 перестраивает старые кэши один раз. Unsaved
факты остаются в памяти, Close восстанавливает disk facts; удаление CSS убирает
определения и подсказки. Правка PHP не перепарсивает CSS. Отключение CSS выключает
class completion/navigation, даже если PHP включён. UI применяет данные только к
точной ревизии и текущему caret; class contexts имеют приоритет над PHP/LSP completion.

Cmd+клик (Ctrl+клик вне macOS) по классу в CSS открывает модальный список всех
индексированных CSS/PHP-мест с файлами, строками, preview и переходом. Из PHP один
selector открывается сразу, несколько — через модальный Choose CSS declaration.
Списки используют общий renderer UsagesPopup: поиск, стрелки, Enter, Escape,
группировка и опциональное отдельное окно. PHP usages сохраняют прежнее поведение.
Hit testing ограничен glyph ranges классов; modifier-hover не открывает CSS popup.
Стандартные CSS-свойства не участвуют в этих переходах и поиске использований.

Проверки: полный native workspace/all-features test последовательно — 270 passed,
0 failed, 9 existing optional tests ignored. Headless UI проверяет реальные Cmd-click,
modal usages/chooser, Enter navigation, ввод свойства/класса, Tab и Undo. Worker tests
проверяют cache/overlay/Close/removal, отсутствие повторного CSS parse при правке PHP,
отключённый CSS, регистр, дефисы, Unicode, escaped selectors, динамические вставки
и исключение false positives. Native Clippy workspace/all-targets/all-features с
-D warnings, rustfmt и git diff --check прошли. Новых зависимостей нет.
Workspace/Cargo.lock/UI/bundle — 0.1.24; следующая доработка — 0.1.25.
Native GUI вручную, Windows build/Clippy, commit и push не выполнялись.

Последняя поправка HTML scanner отдельно проверяет custom elements вроде
script-card и закрывающие raw-text теги без ложного совпадения по префиксу;
focused CSS tests и финальный Clippy прошли. Workspace release build завершён,
штатный update-binaries.py обновил macOS ARM64 executable и подписанный app archive.
--check подтвердил source fingerprint и SHA-256; проверены bundle 0.1.24,
целостность обоих ZIP и точное совпадение четырёх legal sidecars с Resources/Legal.
После проверки выполнен cargo clean (10.7 GiB), удалены временные логи задачи,
Python bytecode packaging script и семь untracked Finder files. Размер проекта —
384 MiB; актуальный macOS-комплект и legal materials сохранены. Windows-кэш
не создавался. Финальные git status и diff --check проверены.

### БД: создание таблиц из дерева объектов (v0.1.25, 2026-11-03)

Узел tables подключённой схемы получил контекстное меню + New Table. Открывается
прежнее окно Modify Table с новым черновиком: те же свойства, колонки, keys,
индексы, триггеры, комментарии и grants. Порядок новых PostgreSQL-колонок можно
менять до создания. Preview и Apply/OK используют creation_sql и CreateTable
на штатном database worker; запросы CREATE не заменяют существующие объекты.
После Apply загружается snapshot, окно переходит к обычному редактированию,
дерево обновляется без переименования или инвалидирования других таблиц.
Cancel не сохраняет черновик. После неудачной попытки создания доступен Reload
для перечитывания таблицы, если MySQL успел применить часть DDL.

SQLite/PostgreSQL создаются в транзакции; MySQL устанавливает engine/collation
и ключи AUTO_INCREMENT при CREATE. Генерация объектов и дополнительных свойств
переиспользует прежний planner. Новых зависимостей нет. Версия workspace,
Cargo.lock, интерфейса и bundle — 0.1.25, следующая доработка — 0.1.26.

Проверки: полный native workspace/all-features test последовательно — 271 passed,
0 failed, 9 existing optional tests ignored. Новый backend regression проверяет
SQL PostgreSQL/MySQL, SQLite CREATE/индекс/STRICT, duplicate name, rollback при
ошибке индекса и предварительную cancellation. Pointer-driven headless UI с реальным
worker проверяет + New Table → форму → CREATE preview → Apply → snapshot/tree,
OK без повторного CREATE и Cancel; тот же тест повторно прошёл после финальной
правки refresh. Native Clippy workspace/all-targets/all-features с -D warnings,
rustfmt, workspace release build и git diff --check прошли. Первичный sandbox
прогон блокировал localhost/process fixtures; полный прогон с системным доступом
успешен. Native GUI вручную и live PostgreSQL/MySQL, Windows build/Clippy,
commit и push не выполнялись.

Штатный update-binaries.py обновил macOS ARM64 executable и подписанный app archive;
--check подтвердил fingerprint исходников и SHA-256 payload. Проверены bundle
0.1.25, целостность ZIP и точное совпадение четырёх legal sidecars с Resources/Legal.
После проверок cargo clean удалил 8.9 GiB; удалены временные логи задачи и untracked
Finder files. Размер проекта после очистки — 385 MiB; готовый macOS-комплект сохранён.
Windows-кэш не создавался. Финальные git status и diff --check проверены.


### YAML/YML: подсветка и безопасное форматирование (v0.1.26, 2026-11-03)

YAML доступен через общий реестр языков для .yaml/.yml без учёта регистра.
Tree-sitter YAML работает в syntax worker: ключи, строки, числа, boolean/null,
комментарии, anchors/aliases, tags и directives. Ключи имеют приоритет над
общими scalar captures. YAML не создаёт semantic index/cache и не запускает
PHP LSP/stubs. В старых проектах с сохранённым opt-in YAML нужно включить вручную.

Reformat Code использует общий YAML-профиль (два пробела) и прежние revision/style
gates, selection и единую Undo-транзакцию. Отступы коллекций выводятся из AST;
изменяются только leading/separator whitespace. После выбранных edits проверяются
AST и scalar/token signatures всего документа. Неоднозначные исправления malformed
YAML и изменения смысла частичных выделений отклоняются. Кавычки, комментарии,
anchors/aliases, LF/CRLF и содержимое скаляров сохраняются. Учтены trailing blank
lines, отсутствующие в AST block_scalar даже при keep chomping (|+).
Enter копирует текущий отступ, paste сохраняет исходные байты. Tabs profile
отклоняется; автоматическое исправление ошибочной иерархии не выполняется.

Новая зависимость — tree-sitter-yaml 0.7.2 (MIT); legal inventory формируется
штатным packaging script. Workspace/Cargo.lock/UI/bundle — 0.1.26, следующая
доработка — 0.1.27.

Проверки: native workspace/all-features последовательно — 278 passed, 0 failed,
9 existing optional tests ignored. Rustfmt, Clippy workspace/all-targets/all-features
с -D warnings и git diff --check прошли. YAML regressions проверяют pipelines,
вложенные/compact sequences, flow collections, anchors/aliases, Unicode, CRLF,
explicit/folded scalars и trailing blank payload, idempotence и безопасный selection.
Worker regression работает с отсутствующим PHP server; YAML-only indexer не создаёт
.nozhik/cache. Headless UI проверяет реальную команду Reformat Code, highlights
и атомарный Undo. Дополнительный независимый audit через Ruby Psych подтвердил
сохранение значений 18 YAML-документов. Native GUI вручную, Windows build/Clippy,
commit и push не выполнялись.

Workspace release build и штатный update-binaries.py завершены; macOS ARM64
executable и подписанный app archive обновлены. --check подтвердил fingerprint
и SHA-256. Проверены bundle 0.1.26, целостность обоих ZIP, codesign --verify
--deep --strict и точное совпадение четырёх legal sidecars с Resources/Legal,
включая notices tree-sitter-yaml. После проверки cargo clean удалил 10.7 GiB.
Удалены временные файлы/логи задачи, packaging bytecode и untracked Finder file.
Размер проекта после очистки — 385 MiB; готовый macOS-комплект сохранён.
Windows-кэш не создавался. Финальные git status и diff --check проверены.

### Docker Compose: containers, logs and consoles (v0.1.27, 2026-11-03)

Added the 15th crate, nozhik-docker: a UI/runtime-independent worker for Compose
discovery, YAML run markers and direct Docker CLI commands. Conventional names
and dotted variants support yaml/yml without following symlinks or scanning
build/dependency caches. Tree/editor tabs use a vector whale icon; the Docker
rail button sits between Database and Notifications and requires Compose files.

AST gutter markers start all services or one service with dependencies. Exact
BufferId/revision and saved-configuration gates reject stale/dirty sources.
YAML semantic indexing and PHP services are not involved. Configuration selection
combines base + conventional override or base + an explicit variant. Compose
evaluates .env/includes/project names. CLI argv never passes through a shell.
Project and canonical working-directory labels isolate every container; foreign
project-name collisions fail before mutation. Commands target validated hex IDs.

The panel includes declared/not-created services and all scoped containers:
Up all/service, Start/Stop existing, Stop All, Refresh and Cancel. Containers and
volumes are never implicitly removed. Info shows ID/image/status/mounts/labels.
Logs retain 500 timestamped lines with stdout/stderr sharing one ordered pipe,
refreshed every three seconds while visible. Console uses the existing PTY with
docker exec -it -- ID /bin/sh in a [Docker: name] tab. Exited Docker tabs retain
output/errors. Closing a console does not stop the container. Worker CLI output
and time are bounded. Cancellation terminates the owned CLI process tree,
including Compose plugins, without signalling daemon/container processes.

No new third-party packages: reuse tree-sitter-yaml, ignore, JSON and Unix nix
signals. Workspace/Cargo.lock/UI/bundle are 0.1.27; next patch is 0.1.28.

Process-isolated mock tests cover names/variants, AST markers, NDJSON/array JSON,
scope isolation, lifecycle commands, stdout/stderr ordering, console argv and
cancellation of a child plugin. Pointer-driven headless UI covers gutter all and
service clicks, dirty gates, rail enablement, Info/Logs/console, named PTY output
and keyboard input. Live Docker smoke passed twice on engine 29.4.0 with a cached
BusyBox image: disposable Compose projects, no ports/mounts/pulls, lifecycle,
logs and exec. Both projects were removed; docker ps confirmed no test containers
remain. User containers and images were unchanged.

Rustfmt, native workspace/all-targets/all-features Clippy with -D warnings,
workspace release build and git diff --check passed. During concurrent release
compilation, an existing Ollama lifecycle test once exceeded its 3s timeout;
the sequential rerun without competing compilation passes that test. Native GUI,
Windows build/Clippy, commit and push were not performed.

The standard update-binaries.py refreshed macOS ARM64 executable and signed app
archive. --check verified source fingerprint and SHA-256. Bundle 0.1.27, both ZIP
archives, codesign --verify --deep --strict and all four embedded legal sidecars
were verified.

Final sequential native workspace/all-features result: 283 passed, 0 failed,
10 optional tests ignored (including the separately executed Docker smoke).
After binary verification cargo clean removed 10.6 GiB; task logs, packaging
bytecode and seven untracked Finder files were removed. Project size is 386 MiB;
the current macOS package and legal materials remain. No Windows target cache
was created. Final git status and diff --check were inspected.

### Контекстное меню дерева проекта (v0.1.28, 2026-11-03)

Правый клик по файлу или директории, включая корень проекта, открывает
Show in File Manager. macOS использует Finder с выделением цели (open -R),
Windows — Explorer /select, Linux — xdg-open родительской директории.
Для директорий доступны New Folder / New File с вводом одного имени;
новый файл сразу открывается через прежний асинхронный read/editor путь.
Создание автоматически раскрывает родителя, обновляет дерево, поиск и Git.

CreateEntry выполняется на serial filesystem worker: валидация имени,
canonical parent внутри проекта и эксклюзивное создание без перезаписи.
EntryCreationError сохраняет имя и ошибку в project-local модальном окне,
возвращает фокус к вводу и не затрагивает save state. Повторное создание
заблокировано на время запроса. После успешной мутации ошибка перечитывания
дерева не превращается в предложение повторить создание. Новых зависимостей нет.
Workspace/Cargo.lock/UI/bundle — 0.1.28, следующая доработка — 0.1.29.

Проверки: полный native workspace/all-features последовательно — 285 passed,
0 failed, 10 optional tests ignored. Backend regression проверяет Unicode/пробелы,
дубликаты с сохранением содержимого, недопустимые имена, выход за пределы проекта,
отсутствующий и ленивый родитель. Headless UI через реальные pointer/text events
проверяет разные меню файлов/директорий и создание файла с открытием и папки.
Native Clippy workspace/all-targets/all-features с -D warnings, rustfmt и
git diff --check прошли. Sandbox ограничивал localhost/process fixtures;
полный повторный прогон с системным доступом прошёл. Native GUI и выделение
в Finder вручную, Windows build/Clippy, commit и push не выполнялись.

Workspace release build и штатный update-binaries.py прошли. macOS ARM64
executable и подписанный app archive обновлены; --check подтвердил fingerprint
исходников и SHA-256 payload. Проверены bundle 0.1.28, ZIP integrity,
codesign --verify --deep --strict и совпадение embedded legal materials.
После проверки cargo clean удалил 8.3 GiB; удалены логи задачи, packaging bytecode
и шесть untracked Finder files. Размер проекта после очистки — 376 MiB;
готовый macOS-комплект сохранён. Windows-кэш не создавался.
Финальные git status и git diff --check проверены.

### Docker: прелоадер только для действий пользователя (v0.1.29, 2026-11-03)

Периодическое обновление контейнеров/логов выполняется без прелоадера.
Явные запросы из UI (Refresh, выбор конфигурации, запуск/остановка, Logs,
Open console и gutter actions) отмечаются отдельно при принятии команды worker.
Индикатор отображается только для такого активного запроса, в постоянном месте
строки кнопок панели и окна контейнера. Cancel постоянно присутствует в панели
и активен во время запроса. Удалена условная строка Docker operation над списком:
появление и исчезновение индикатора больше не меняет положение контейнеров.
Ошибки и отмена сохраняют прежнее поведение. Новых зависимостей нет.
Workspace/Cargo.lock/UI/bundle — 0.1.29; следующая доработка — 0.1.30.

Проверки: native workspace/all-features последовательно — 287 passed, 0 failed,
10 прежних optional tests ignored. Headless regression сравнивает положение
списка при idle/background/user requests и ширине 180/280/420 px; отдельный тест
проверяет фактическую отрисовку spinner только для пользовательского запроса.
Rustfmt и native Clippy workspace/all-targets/all-features с -D warnings прошли.
Первый sandbox-прогон блокировал localhost fixtures; полный повторный прогон
с системным доступом прошёл. Native GUI вручную, Windows build/Clippy, commit
и push не выполнялись.

Workspace release build и штатный update-binaries.py прошли. Обновлены macOS
ARM64 executable и подписанный Nozhik.app.zip; --check подтвердил fingerprint
и SHA-256. Проверены bundle 0.1.29, ZIP integrity и все четыре embedded legal
sidecars. После проверки cargo clean удалил 7.2 GiB; удалены временные логи
и packaging bytecode. Размер проекта — 375 MiB; готовый macOS-комплект сохранён.
Windows-кэш не создавался. Финальные git status и git diff --check проверены.

### Память PHP/CSS-индексов (v0.1.30, 2026-11-03)

Устранено размножение полных строк минифицированного CSS/PHP в preview каждого
символа. Общий extractor сохраняет максимум 512 UTF-8 байт вокруг символа;
preview_offset отделён от исходных byte/line/column, поэтому подсветка usages
и точные переходы работают после обрезки. Preview хранится в Arc<str>, FileSymbols
разделяется через Arc. Disk/live/UI разделяют неизменённый Arc<SymbolIndex>;
copy-on-write создаёт отдельное состояние только при изменении. После Close
последнего overlay live снова разделяет disk. Неиспользуемый UI CSS SymbolIndex
удалён; класс-каталог остаётся. CSS/markup используют общую таблицу строк файла,
исключая повторное сканирование всего префикса для каждого класса.

Входной mailbox объединяет Overlay/Close по пути, сохраняет независимые Changed
и при потоке более 256 разных путей ставит Reconcile. Выходной mailbox сохраняет
последний снимок каждого вида и последний progress/finished/error; старые Arc
освобождаются до чтения UI. Пакет команд публикует снимки и класс-каталог один раз.
Drop очищает очередь и будит ожидающий worker. Кэши схемы 6 имеют новые имена
index_php_v6.json/index_css_v6.json: старые гигабайтные JSON не десериализуются,
удаляются только после успешной атомарной записи нового кэша своего языка.
Все поддерживаемые источники и несохранённые overlays по-прежнему индексируются.

Read-only аудит CSS реальных проектов (временный отдельный executable удалён):
leadforms — 88 файлов, 71280 классов, старый cache 2403396036 bytes против
29681492 bytes нового сериализованного SymbolIndex; nozhove.com — 13 файлов,
10764 класса, 1857858368 против 8182106 bytes. Число классов сохранено.
Оба новых CSS-индекса, worker/live/UI Arc handles и class catalogs одновременно
удерживались процессом: peak RSS 147996672 bytes (~141 MiB), 3.21 s wall time
в debug build. Это CSS-only измерение, не расход всей IDE. Исходники и кэши
пользовательских проектов при аудите не изменялись. Первичный измерительный
процесс остановлен перед оптимизацией таблицы строк; финальный замер завершён.

Undo проверен по реализации: история использует copy-on-write Rope и edits.
Автосохранение не устраняет рабочий текст/семантический индекс и не заменяет Undo
через Git. mark_saved сейчас завершает typing group; запись каждого символа
добавила бы filesystem/Git события и изменила Undo. Поведение сохранения и истории
в этой доработке сохранено; autosave с debounce/local recovery — отдельная функция.

Проверки: полный native workspace/all-features последовательно — 291 passed,
0 failed, 10 прежних optional tests ignored. Новые regressions проверяют bounded
minified CSS/PHP/Unicode previews и точные координаты, sharing/COW/Close, пропуск
legacy JSON и безопасную миграцию, очереди из 1000 обновлений и освобождение
устаревших snapshots. Existing UI usages/completion/navigation/Undo и project
switching tests проходят. Первый полный прогон нашёл старое имя кэша в UI-тесте
настроек; после обновления ожидания полный прогон прошёл. Native Clippy
workspace/all-targets/all-features с -D warnings и rustfmt прошли.
Новых пакетов нет; serde включён с rc для сериализации разделяемых данных.
Workspace/Cargo.lock/UI/bundle — 0.1.30; следующая доработка — 0.1.31.
Windows build/Clippy, ручной native GUI, commit и push не выполнялись.

Workspace release build и штатный update-binaries.py прошли. Обновлён macOS ARM64
комплект; --check подтвердил source fingerprint/SHA-256. Проверены bundle 0.1.30,
оба ZIP, codesign --verify --deep --strict и совпадение четырёх embedded legal
materials. После проверок cargo clean удалил 9.4 GiB; временные diagnostic example,
логи, executable в target и packaging bytecode удалены. Размер проекта после
очистки — 376 MiB; готовые бинарники/legal materials сохранены. Windows-кэш
не создавался. Предыдущие незакоммиченные Docker-изменения сохранены.
Финальные git status и git diff --check проверены.

### Локальная LLM-консоль (v0.1.31, 2026-11-03)

Новая кнопка LLM console (робот) рядом с Terminal открывает отдельную локальную
PTY-панель проекта с независимыми вкладками. После запуска grok/codex Unix worker
определяет точный executable/PID foreground-процесса; macOS также проверяет
foreground-группу для npm/node-обёрток. Вывод и OSC title не используются для
распознавания. На Windows панель пока работает как обычный терминал: backend
автоопределения/контекста не подключён. Удалённые SSH/Docker агенты исключены.

Распознанный агент получает Open files / Selection / Attach file и локальный
drag-and-drop в область панели. Открытые текстовые буферы передаются UTF-8-снимками,
включая несохранённые изменения, с mapping к исходным путям/revision. SFTP/binary
вкладки исключены. Выделения сохраняют текст, исходный путь, revision, half-open
byte offsets и 1-based UTF-8 byte line/column; смена фокуса не меняет cursors.
Копии вложений со стабильными именами и snapshots принадлежат отдельному worker
и приватному TempDir. Пределы: 20 MiB/file, 16 файлов/drop, 256 MiB/panel,
64 KiB/selection paste. При закрытии проекта/IDE worker удаляет временные копии
после остановки терминалов; hide/reopen сохраняет сессии и контекст.

Изображения идут отдельным bracketed paste локального пути: CLI распознаёт
нативное вложение. Другие файлы идут ссылкой с просьбой прочитать их средствами
агента. Enter не добавляется. Асинхронный ответ связан с tab ID и agent PID/type;
при смене вкладки/процесса он отклоняется. Control worker повторно проверяет
foreground и bracketed paste, writer проверяет процесс непосредственно после
ранее поставленных клавиатурных байтов. Терминальные control characters в
контексте отклоняются. Нет новых пакетов, API-интеграции или хранения credentials.

Проверки: native workspace/all-features — 296 passed, 0 failed, 10 прежних optional
ignored. Регрессии проверяют PTY-распознавание, реальную вставку и stale PID reject,
точные Unicode/reverse-selection координаты, несохранённые snapshots без записи
в исходник, lifetime копий, size/control limits, scoped enabled drag-and-drop,
реальные rail clicks LLM/Terminal/Problems. Native Clippy workspace/all-targets/
all-features с -D warnings, rustfmt и git diff --check прошли. Первый sandbox
прогон блокировал localhost fixtures; полный повтор с системным доступом прошёл.

Установленный Codex 0.166.0 проверен в настоящем PTY: тестовый PNG стал [Image #1]
в черновике без отправки модели. Grok 1.0.67 распознан как foreground executable,
но его TUI потребовал browser login: авторизация и запрос модели не выполнялись.
Механизм Grok image-path paste проверен по официальным upstream исходникам.
Тестовый вход остановлен. Ручная native GUI-проверка Nozhik, Windows build/Clippy,
commit и push не выполнялись. Версия workspace/Cargo.lock — 0.1.31, следующая 0.1.32.

Workspace release build и штатный update-binaries.py прошли. Обновлён macOS ARM64
комплект; --check подтвердил source fingerprint/SHA-256. Проверены bundle 0.1.31,
целостность обоих ZIP, codesign --verify --deep --strict и совпадение всех четырёх
embedded legal sidecars. После проверок cargo clean удалил 8.9 GiB; удалены логи
задачи, скачанные upstream references, PTY smoke artifacts, packaging bytecode и
восемь untracked Finder files. Тестовые процессы Grok/Python завершены. Размер
проекта после очистки — 376 MiB; готовые бинарники/legal materials сохранены.
Windows-кэш не создавался, Windows-бинарники не обновлялись. Финальные git status
и git diff --check проверены.

Пользователь вручную проверил LLM-консоль с Grok Build и подтвердил корректную
работу всего функционала: контекст редактора, файлы и скриншоты.

### Мышь, выделение и Unicode LLM-терминала (v0.1.32, 2026-11-04)

TerminalGrid передаёт negotiated mouse mode/encoding. UI отправляет точные
press/release, drag/motion и wheel координаты в xterm SGR/UTF-8/X10; учитывает
модификаторы, границы ячеек, отпускание за пределами области и блокирующие модальные
окна. Shift оставляет локальное выделение; правый клик при native capture получает
агент, в локальном режиме остаётся Copy/Cut/Paste. Wheel/trackpad дроби и capture
разделены по сессиям. Alternate-screen программы без mouse reporting получают
стрелки колёсом. Кнопки и ссылки Grok обслуживаются его родными обработчиками.

Для большого копирования добавлены Text selection, Copy all и Live · Esc.
Review лениво клонирует ограниченный vt100 Screen; живой parser и процесс
продолжают работать отдельно. History scrollbar, wheel, Shift+PageUp/PageDown
и drag auto-scroll сохраняют абсолютные endpoints выделения между страницами.
Worker CopyText проверяет epoch, читает весь диапазон (до 4 MiB), склеивает soft
wrap и возвращает отдельный Copied event. Выход в live/resize освобождают frozen
snapshot и инвалидируют диапазон. Нормальная история ограничена 3000 строками;
локальная копия alternate screen содержит только видимый экран. Внутреннюю
многостраничную историю Grok выделяет и копирует сам через native mouse.

Поддержан резервный OSC 52 copy, который использует Grok после drag/release.
Только UTF-8, до 4 MiB, только запись системного clipboard и только в течение
5 секунд после явного native mouse input. Clipboard-read queries не получают
ответа; unsolicited OSC 52 отбрасывается. Обработка не держит shared locks при
публикации событий. base64 0.22.1 — новая прямая зависимость terminal, пакет уже
был в Cargo.lock; других новых Rust-пакетов нет.

Отдельное семейство Terminal: JetBrains Mono + Noto Sans Symbols 2 + современный
Noto Emoji + прежние egui fallbacks. Устранены квадраты для Braille логотипа,
Powerline/стрелок/блоков и обычных emoji. Fallback glyphs вписываются/обрезаются
по terminal cell без изменения исходного Unicode в clipboard. Оба добавленных
шрифта включены без модификаций; upstream OFL сохранены и встроены в legal
sidecars. Обновлены README, architecture, coding standards и версия/lock —
0.1.32; следующая доработка — 0.1.33.

Проверки: native workspace/all-features последовательно — 302 passed, 0 failed,
10 прежних optional ignored. Регрессии проверяют настоящие egui pointer/wheel
события и точные SGR bytes, Shift override/modal gates, negotiated modes, большой
frozen copy во время живого вывода, soft wraps/stale epochs, gated OSC 52 и запрет
clipboard reads, покрытие всех 256 Braille patterns, Powerline и Unicode/emoji.
Параллельный прогон выявлял прежний нестабильный SQL UI-тест Missing UI label:
edited; изолированный тест и полный последовательный прогон прошли без изменения
SQL-кода. Native Clippy workspace/all-targets/all-features с -D warnings,
rustfmt и git diff --check прошли. Ручной native GUI и вход/запрос к настоящим
LLM в этой задаче не выполнялись; protocol fixtures не используют аккаунты,
модели или пользовательские проекты. Windows build/Clippy, commit и push
не выполнялись.

Workspace release build и штатный update-binaries.py прошли. Обновлены macOS ARM64
executable/app archive; --check подтвердил source fingerprint и SHA-256. Проверены
bundle 0.1.32, оба ZIP, codesign --verify --deep --strict, все четыре embedded
legal sidecars и наличие обоих новых font OFL в поставке. После проверок cargo
clean удалил 9.7 GiB; удалены временные upstream downloads, логи задачи, packaging
bytecode и шесть untracked Finder files. Размер проекта после очистки — 383 MiB.
Готовый macOS комплект сохранён; Windows-кэш не создавался, Windows-комплект
не обновлялся. Финальные git status и git diff --check проверены.

### SFTP Deployment проекта (v0.1.33, 2026-11-04)

Settings → SFTP Deployment хранит один optional target проекта: имя уже сохранённого
SSH-соединения и абсолютный POSIX remote project root. Apply deployment валидирует
черновик и сохраняет через прежний project settings worker; Disable deployment
убирает mapping. Старые TOML/JSON без поля по-прежнему загружаются. Секреты не
дублируются: используются прежние host key trust, пароль/ключ/agent и OS store.

После настройки в контекстном меню файла/директории/корня проекта появляется
SFTP Deploy. Корень переносится без дополнительной директории имени проекта;
подкаталоги и файлы сохраняют project-relative paths. Нужные и пустые каталоги
создаются на сервере; файлы передаются как binary streams по 64 KiB, без лимита
редактора 16 MiB. Совпадающие обычные файлы перезаписываются с truncation; лишние
удалённые файлы сохраняются. .git/.nozhik и вложенные локальные симлинки исключены,
явно выбранные internals/симлинки/outside paths отклоняются. Remote root разрешён
через realpath; внутренние целевые симлинки/неверные типы блокируют запись.

Deployment intent RemoteWorker переиспользует Verify/Trust/Connect и отдаёт
проверенный SshSession отдельному worker передачи. Scan/metadata/file I/O и SFTP
не выполняются в UI; transfer не ограничен общим 35-second SSH-command timeout,
но использует deadlines отдельных операций. План ограничен 100000 entries,
данные файлов не материализуются целиком. Latest-only progress (files/bytes/path)
будит окно не чаще 100 ms плюс completion, показан над деревом вместе с Cancel.
Cancel/закрытие проекта прерывают работу; уже загруженное остаётся, текущий файл
может быть частично записан. Rollback, удаление remote extras и atomic deploy
не реализуются. Независимый SFTP browser обновляется своим Refresh.

До запроса и после SSH auth проверяются сохранённые локальные buffers выбранного
поддерева. Pending ID связан с root/selection/config/profile identity; изменение
настроек/профиля при ожидании не перенаправляет запрос. После отмены stale
Trust/Verified/Credentials replies не открывают диалоги. DeploymentFailed не
сбрасывает editor save state. Активные и фоновые проекты читают completion,
shutdown ограничен и предшествует остановке SSH transport.

Проверки: native workspace/all-features последовательно — 309 passed, 0 failed,
10 прежних optional ignored. Новые regressions: обе сериализации и legacy defaults,
реальные pointer clicks настройки/выбора SSH/Apply/Disable, conditional file/dir
context menu с точной целью, dirty gate/cancel/stale auth. Тестовый SFTP v3 server
на in-memory duplex проверяет mkdir/empty directories, Unicode names, многочанковый
binary upload, truncating overwrite, сохранение unrelated files, отказ для remote
symlink и PermissionDenied. Planner проверяет локальные exclusions/containment/
symlinks и cancellation. Native Clippy workspace/all-targets/all-features с
-D warnings, rustfmt, git diff --check и workspace release build прошли.
Реальный пользовательский сервер/credentials и ручной native GUI не использовались.
Новых зависимостей нет. Workspace/Cargo.lock/UI — 0.1.33; следующая 0.1.34.
Windows build/Clippy, commit и push не выполнялись.

Штатный update-binaries.py обновил macOS ARM64 executable и подписанный app archive;
--check подтвердил source fingerprint/SHA-256. Проверены bundle 0.1.33, оба ZIP,
codesign --verify --deep --strict и совпадение четырёх embedded legal sidecars.
После проверок cargo clean удалил 9.4 GiB; удалены task logs, временный файл,
packaging bytecode и шесть untracked Finder files. Размер проекта после очистки —
386 MiB; готовые бинарники/legal materials сохранены. Windows-кэш не создавался,
Windows-комплект не обновлялся. Финальные git status и git diff --check проверены.


### SFTP Deploy Files из Git History (v0.1.34, 2026-11-04)

При настроенном deployment Git → History показывает SFTP Deploy Files справа
от Pull/Push и меню. Под кнопкой постоянное пояснение: коммит определяет только
список путей, отправляются текущие файлы проекта. Требуется один выбранный коммит
с готовыми details. Список захватывается при клике; исторические blobs не читаются.

Точный batch-план deduplicate-ит пути, не раскрывает каталоги и пропускает missing,
internal, symlink, directory и outside-project entries. Отсутствующие удаления не
удаляют серверные файлы; если подходящих файлов нет, выводится ошибка. Dirty gate
требует сохранения только включённых файлов, включая повторную проверку после SSH.
Общие проверка host key/auth, потоковая передача, прогресс и отмена переиспользованы;
прогресс/отмена видны также в Git. Dropdown remote actions — квадратная часть
панели высотой 50 px без рамки/скругления и разделителя после Pull/Push.

Добавлены pointer-driven UI test выбора коммита/клика/списка путей и геометрии меню,
проверка точного batch-плана без рекурсии с exclusions/dedup/cancellation, dirty
included/unrelated files. SFTP protocol fixture отправляет изменённые текущие bytes
через batch API, пропускает отсутствующий путь и сохраняет unrelated remote files.
Новых зависимостей нет. Workspace/Cargo.lock/UI — 0.1.34; следующая 0.1.35.


Проверки v0.1.34: cargo fmt --all -- --check; cargo clippy --workspace
--all-targets --all-features -- -D warnings; cargo test --workspace --all-features
-- --test-threads=1 — 311 passed, 10 ignored; cargo build --workspace --release.
Для loopback/PTY fixtures тесты запущены вне sandbox; один стартовый lifecycle
тест при первой параллельной компиляции превысил deadline, повторный полный прогон
прошёл. Реальный пользовательский сервер и ручное GUI-окно не использовались.
python3 scripts/update-binaries.py обновил native macOS ARM64, --check подтвердил
source fingerprint и checksums. Проверены версия bundle 0.1.34, целостность обоих
ZIP, legal sidecars в Resources/Legal и codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки бинарников cargo clean удалил 8.2 GiB; удалены временные журналы,
packaging bytecode и семь untracked Finder files. Размер проекта — 386 MiB;
исходники, незакоммиченные правки и готовый комплект сохранены. Финальные
размер/git status/git diff --check проверены.

### JSON и каталог Tools (v0.1.35, 2026-11-04)

.json (включая .JSON) получает лексическую подсветку ключей, строк, чисел,
boolean/null и пунктуации. Reformat Code / Reformat JSON форматирует весь файл
или самостоятельное JSON-значение в выделении по профилю JSON из File Types.
Учитываются отступ окружающего блока, выбранные spaces/tabs и CRLF. Форматтер
валидирует через IgnoredAny и меняет только whitespace: порядок/повторы ключей,
Unicode/escapes и числовые lexemes, включая большие экспоненты, сохраняются.
JSON не входит в semantic index, не получает PHP stubs и не запускает PHP LSP.

Tools в левом rail и верхнем меню открывает каталог инструментов. JSON из каталога
открывает отдельное плавающее окно, которое остаётся открытым после закрытия
каталога. Два активных поля Minified JSON / Formatted JSON синхронизируются после
валидных правок и автоматически форматируются; у каждого есть Copy. Clear очищает
оба. Ошибочный/незавершённый ввод сохраняется с подсветкой и ошибкой; другое поле
хранит последний корректный результат. Background worker имеет bounded input queue,
revision gate, callback wake; cursor/selection remap сохраняет позицию в токенах.
Лимит ввода — 2 MiB. Текст принадлежит сессии проекта, не сохраняется на диск.

Open in Tools → JSON в редакторе переносит текущий буфер, включая несохранённый
текст; исходный файл инструмент не меняет. Новая регистрация сохраняет прежний
порядок языков в компактных Settings. Версия workspace/lock/UI/bundle — 0.1.35;
следующая доработка — 0.1.36. Новых зависимостей нет.

Проверки: cargo fmt --all -- --check; native Clippy workspace/all-targets/all-features
с -D warnings; cargo test --workspace --all-features -- --test-threads=1 — 318 passed,
0 failed, 10 прежних optional ignored; cargo build --workspace --release. Новый
pointer/keyboard UI test открывает JSON из файла, вставляет текст в оба поля,
проверяет Copy и сохранность исходника; отдельные тесты проверяют каталог/отдельное
окно, синхронизацию, malformed/stale replies, Unicode cursor mapping, size limit,
подсветку файла, atomic Undo, lossless formatting, profiles/ranges/CRLF. Полный
sandbox прогон блокировал localhost/PTY fixtures; итоговый прогон с системным
доступом прошёл. Native GUI вручную и Windows build/Clippy не запускались.

Штатный update-binaries.py обновил macOS ARM64; --check подтвердил source fingerprint
и SHA-256. Проверены версия bundle 0.1.35, ZIP integrity и четыре embedded legal
sidecars; упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся. Commit и push не выполнялись.

После проверки готового комплекта cargo clean удалил 9.4 GiB; удалены task logs
и шесть untracked Finder files. Финальный размер проекта — 387 MiB, binaries —
108 MiB. Исходники, незакоммиченные правки, история и готовые бинарники/legal
materials сохранены. После очистки сборки не запускались; native --check,
git diff --check, размер и git status проверены.

### Исправление JSON и полноразмерный Tools (v0.1.36, 2026-11-04)

Старые project settings с явно сохранённым списком PHP/CSS/YAML исключали JSON
из language worker, поэтому JSON открывался без подсветки и Reformat не работал.
ConfigManager::read теперь мигрирует непустой legacy список: добавляет JSON и
устанавливает language_catalog_version=1 в памяти. Пустой список не меняется;
явное отключение JSON после миграции сохраняется при reload. Безуспешный разбор
настроек по-прежнему не меняет файл; сама миграция не выполняет запись на диск.

Tools — самостоятельный раздел CentralPanel с каталогом инструментов вместо
плавающего окна. Выбранный JSON занимает ту же полноразмерную рабочую область,
Back to Tools возвращает каталог. Rail/menu открывают каталог; перенос из файла
сразу открывает JSON. Hidden file editor исключён из action_context.editor_open,
чтобы команды закрытия/форматирования не меняли скрытый файл. Явное открытие файла
возвращает файловый редактор. Scratch buffers сохраняются при смене разделов.

Minified JSON находится сверху, Formatted JSON — снизу; между ними draggable
разделитель. Устранено centered overflow от add_sized: TextEdit теперь расположен
слева сверху, независимо прокручивается в ограниченном ScrollArea. Начало длинных
строк больше не оказывается левее поля. Перенос/очистка сбрасывают просмотр и
курсор к началу. Замена стрелки в Open in Tools / JSON устранила missing glyph.
Синхронизация, фоновые revision gates, подсветка и редактирование обоих полей
сохранены. JSON по-прежнему не индексируется. Новых зависимостей нет.

Проверки native: fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=1 — 320 passed, 0 failed,
10 прежних optional ignored; cargo build --workspace --release. Добавлены legacy
migration tests обеих сериализаций/явного opt-out, проверка реальной раскраски
файлового редактора после форматирования и геометрии длинных строк обоих полей
при 1400×900 и 800×600. Каталог/возврат, ввод/Copy/перенос проверены pointer/keyboard
UI-тестами. Первый полный прогон совпал с release compilation и превысил deadline
прежнего lifecycle fixture; итоговый прогон после сборки прошёл. В новом тесте
отрисовки исправлено ожидание точной revision после форматирования.

macOS GUI запущен через временный app wrapper с отдельными settings и проектом
в /tmp, без пользовательских данных и AI. Визуально подтверждены подсветка JSON
со старым списком языков, перенос длинного JSON, начало обоих полей, расположение
сверху вниз, полноразмерные JSON/каталог и возврат. Тестовое приложение закрыто
Cmd+Q. Штатный update-binaries.py обновил macOS ARM64 executable/app archive до
0.1.36; --check проверяет source fingerprint/SHA-256. ZIP integrity, bundle version,
четыре legal sidecars и codesign --verify --deep --strict проверены. Windows
build/Clippy и Windows-комплект не выполнялись/не обновлялись. Commit/push нет.

После проверок cargo clean удалил 8.6 GiB; удалены временный GUI-проект/app wrapper,
журналы задачи и семь untracked Finder files. Размер проекта — 387 MiB, binaries —
108 MiB. Готовый native комплект, legal materials, исходники, история и прежние
незакоммиченные правки сохранены. После очистки сборок не было; финальные native
--check, git diff --check, размер и git status проверены.

### URL и Base64 encode/decode в Tools (v0.1.37, 2026-11-04)

Каталог Tools теперь содержит JSON, URL encode / decode и Base64 encode / decode.
Оба новых инструмента используют прежнюю полноразмерную рабочую область: Decoded
text сверху, Encoded text снизу, регулируемый разделитель и независимая прокрутка.
Оба поля активны, Copy есть у каждого; Clear очищает только текущий инструмент.
Правки автоматически пересчитывают противоположное поле; вводимая сторона не
нормализуется, поэтому сохраняются её точные байты и курсор. Ошибочный/незавершённый
ввод сохраняется вместе с последним корректным результатом другого поля.

URL использует UTF-8 percent encoding для component (пробел=%20, плюс=%2B,
unreserved -._~). Checkbox Form encoding (+ for spaces) включает form-urlencoded:
пробел=+, буквальный плюс=%2B, * без escaping, ~=%7E. Декодирование строго проверяет
%HH и UTF-8; в component-режиме '+' остаётся плюсом. Base64 использует стандартный
алфавит и padding при encode. Decode принимает также отсутствие padding и ASCII
whitespace/переносы, отклоняет неверные символы/trailing bits и non-UTF-8 output.
Текст не отправляется в сеть и не сохраняется на диск; лимит ввода — 2 MiB.

Tools теперь хранит выбор ToolKind и три независимых ToolEditor. Разделены тексты,
ошибки, revisions, bounded worker queues, last-edited side/mode и split ratios;
egui IDs включают ToolKind для независимых focus/selection/undo/scroll. Возврат
в каталог сохраняет scratch buffers. URL-mode входит в snapshot job; stale replies
и Clear не могут перезаписать новый ввод. Файловый перенос по-прежнему выбирает
JSON; его форматирование, подсветка, cursor remap и semantic-index policy сохранены.

UI переиспользует workspace base64 0.22.1 — пакет уже поставлялся через terminal,
MIT OR Apache-2.0 разрешает такое использование. Новых Rust-пакетов нет; Cargo.lock
получил только workspace patch и прямую dependency UI на прежний base64. Версия
workspace/lock — 0.1.37, следующая — 0.1.38. README/architecture/coding standards
обновлены.

Проверки: native fmt, Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=1 — 324 passed, 0 failed,
10 прежних optional ignored. Полный прогон с системным доступом использует только
изолированные localhost/PTY fixtures. Codecs tests проверяют Unicode/emoji/NUL,
standard Base64 vectors, unpadded/multiline input, form/component semantics,
malformed escapes/Base64/non-UTF-8. Pointer/keyboard UI tests открывают оба пункта
каталога, вставляют текст в обе стороны, нажимают обе Copy, удаляют ввод, проверяют
mode checkbox, stale replies, независимость buffers и горизонтальную геометрию
полей сверху вниз. Ручной native GUI и Windows build/Clippy в этой задаче не
запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable/app archive; --check подтвердил source fingerprint и SHA-256. Проверены
bundle 0.1.37, целостность обоих ZIP, четыре embedded legal sidecars и наличие
base64 в лицензиях. Упаковщик выполнил codesign --verify --deep --strict. Сборки
и проверки использовали локально закэшированные зависимости (--offline), новых
скачиваний не требовалось. Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 8.2 GiB; журналы задачи
удалены, untracked Finder files не обнаружены. Финальный размер проекта — 388 MiB,
binaries — 108 MiB. Исходники, история и готовые бинарники/legal materials сохранены;
после очистки сборок не было. Финальные native --check, git diff --check, размер
и git status проверены.

### Problems и уведомления сессии (v0.1.38, 2026-11-05)

Кнопка Problems открывает журнал ошибок текущей сессии проекта. Внутри одна
раскрывающаяся вкладка Project с числом проблем. В неё попадают общие сбои,
включая неверную авторизацию Git, подключение к базе, SSH, SFTP, Docker,
файловый watcher, форматтер, индексацию, терминал и сервис AI. Другие категории
можно добавить позже, не меняя журнал.

Новая проблема появляется справа снизу. Одновременно видны три последних окна;
остальные ждут в полосе «N more» и не стареют. Info — серая карточка и синяя
«i», Warning — тёплая и оранжевый «!», Error — красноватая, Critical — красная
заливка. Окно держится 3 секунды и за 0,5 секунды исчезает. Наведение
останавливает таймер. Крестик справа сверху закрывает его за 0,2 секунды и не
удаляет запись из списка. Повтор того же сообщения новое окно не открывает.
Журнал не пишется на диск, хранит до 100 записей и не очищается, когда исходное
поле ошибки опустело. «Connection test succeeded» проблемой не считается.

Красные подписи ошибок убраны из панелей открытого проекта. Строка статуса не
рисует текст, который повторяет проблему. Успех и ход операции остаются в
статусе. На месте оставлены ошибка клонирования, история проектов до открытия
папки, текст решения о host key, предупреждение проверки в Tools и счётчики
LSP-диагностик. Новых зависимостей нет. Пока уведомление видно, кадр
запрашивается через request_repaint_after.

Версия workspace/lock — 0.1.38, следующая — 0.1.39. README, architecture и
coding standards описывают контракт.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=1 — 329 passed, 0 failed,
10 прежних optional ignored. Problems-тесты проверяют однократную запись,
счётчик и раскрытие вкладки Project, красную заливку Critical, предел в 100
записей, удержание, плавное скрытие и закрытие крестиком без удаления проблемы.
Ручной native GUI и Windows build/Clippy не запускались. Commit и push не
выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
e9bb0488efbe1dd89e43c4f315af932bc40f26c22870463170c38d8cfbf67764 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.4 GiB. Удалены untracked
Finder .DS_Store. Финальный размер проекта — 388 MiB, binaries — 109 MiB.
Исходники, история и готовые бинарники/legal materials сохранены; после очистки
сборок не было.

### Точные сообщения коммитов (v0.1.39, 2026-11-05)

Автозаполнение больше не отправляет один обрезанный diff. Git worker отдаёт
каждый staged-файл: статус, переименование, счётчики строк, заголовки hunk и
собственный patch до 8 KiB. Общий бюджет текста — 512 KiB, и сокращаются сначала
самые большие файлы. Бинарные файлы остаются в списке без тела.

Локальная модель сначала пишет короткую заметку по небольшой пачке файлов,
затем при длинном коммите сжимает заметки одной директории и только после этого
собирает название и описание. В итоговый запрос не попадает сырой patch.
Непрочитанный файл не исчезает: в заметке остаются статус и имена hunk.
Слишком общий черновик переписывается один раз. Кнопка показывает текущий шаг.
Генерация по-прежнему не создаёт коммит и прерывается Cancel.

Версия workspace/lock — 0.1.39, следующая — 0.1.40. README, architecture и
coding standards описывают контракт. Все 15 packages в Cargo.lock стоят на 0.1.39.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 341 passed,
0 failed, 10 прежних optional ignored. Первый полный прогон остановился на
pointer-тесте кнопки: мок отвечал одним итоговым JSON, а генерация сначала ждёт
заметку по файлу. Мок теперь отвечает обоими шагами и проверяет, что в первый
запрос попадает staged diff без unstaged-текста, а итоговый запрос содержит
заметку файла и не содержит сырой patch. Повторный полный прогон прошёл.
Ручной native GUI и Windows build/Clippy не запускались. Commit и push не
выполнялись. Живой Ollama не вызывался.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
ee36e1ed7abe1b54877d3b5e270f435bca93803e0b94170ad2b3e8d0b86edcb8 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 8.6 GiB. Удалены untracked
Finder .DS_Store. Финальный размер проекта — 389 MiB, binaries — 109 MiB.
Исходники, история и готовые бинарники/legal materials сохранены; после очистки
сборок не было.

### Итог сообщения коммита берётся из заметок (v0.1.40, 2026-11-05)

Проверка на текущих правках вернула в оба поля служебную фразу повторного
запроса: «The previous reply omitted or renamed a path. Copy each required path
unchanged.» Локальная модель скопировала её в заметку или в итоговый JSON, а
прежняя проверка при равном нулевом счёте оставляла этот черновик. Повторный
запрос ещё и вставлял плохой текст обратно в промпт, поэтому модель копировала
его снова.

Заметка, которая является только такой служебной фразой, заменяется статусом
файла и именами hunk. Итоговый ответ принимается, только если название не общее
и не служебное, а описание не повторяет название и не состоит из инструкции.
Иначе форма заполняется самими заметками: описание — их список, название —
строка по областям или по единственной заметке. В повторный запрос плохой
черновик больше не вставляется.

Версия workspace/lock — 0.1.40, следующая — 0.1.41.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings
(пустой список пропущенных областей записан как `Some([])`, без
`redundant_guards`); cargo test --workspace --all-features --offline --
--test-threads=8 — 343 passed, 0 failed, 10 прежних optional ignored. Два новых
теста не дают служебной фразе попасть ни в заметку файла, ни в итог. Ручной
native GUI и Windows build/Clippy не запускались. Commit и push не выполнялись.
Живой Ollama не вызывался.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
69415a9b53545f214cf0256c0b6c69847aabb925f20db9e087015933ca7232f6 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 7.6 GiB. Untracked Finder
.DS_Store не было. Финальный размер проекта — 390 MiB, binaries — 109 MiB.
Исходники, история и готовые бинарники/legal materials сохранены; после очистки
сборок не было.

### Разделитель и прокрутка формы коммита (v0.1.41, 2026-11-05)

Полоска над полями коммита перетаскивается по вертикали и меняет высоту списка
изменённых файлов и формы. Обе части остаются в пределах боковой панели. Если
список или форма не помещаются по высоте или по ширине, у этой части появляется
своя полоса прокрутки: длинный путь, Summary и Description не выталкивают кнопку
Commit за край окна. Summary и Description копируют выделение стандартным
сочетанием копирования. На macOS его забирает системное меню Edit, поэтому
действие Copy читает сфокусированное поле формы раньше diff и редактора.

Версия workspace/lock — 0.1.41, следующая — 0.1.42.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 344 passed,
0 failed, 10 прежних optional ignored. Тест формы коммита проверяет перетаскивание
разделителя, обе полосы прокрутки и копирование выделения. Ручной native GUI и
Windows build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
d38d3563b69d46bf30de7c9a11cb6ece0c091ae46c34708898143c7413599a06 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.4 GiB (29600 файлов).
Untracked Finder .DS_Store удалены. Финальный размер проекта — 390 MiB,
binaries — 109 MiB. Исходники, история и готовые бинарники/legal materials
сохранены; после очистки сборок не было.

### Удалены кнопка и панель Notifications (v0.1.42, 2026-11-05)

Верхняя кнопка Notifications на левой полосе и открываемая ею пустая панель
удалены. Вместе с ними убраны `LeftTool::Notifications` и иконка колокольчика.
Сверху остались Project, Git, Remote, Database, Docker и Tools. Кнопка Docker
по-прежнему стоит между Database и Tools и активна при наличии Compose-файлов.
Повторный клик по оставшимся инструментам скрывает их окно.

Версия workspace/lock — 0.1.42, следующая — 0.1.43.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 345 passed,
0 failed, 10 прежних optional ignored. Headless-тест полосы проверяет, что
шестая верхняя кнопка открывает Tools, следующий слот пуст, а заголовок
Notifications и заглушка панели не рисуются. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
1ee93029fe49c5dc948e8e510e8267568ce56f8b1609341d17118a11d0e20c65 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 7.9 GiB (24882 файла).
Удалён untracked Finder .DS_Store. Финальный размер проекта — 391 MiB,
binaries — 109 MiB. Исходники, история и готовые бинарники/legal materials
сохранены; после очистки сборок не было.

### Заметки Snippets в корне проекта (v0.1.43, 2026-11-05)

В верхней группе левой полосы, между Docker и Tools, появилась кнопка Snippets
с иконкой ярлыка. Раздел занимает то же окно, что и Git: полоса кнопок остаётся,
левая колонка показывает Favorites (All Snippets и Uncategorized) и Tags,
центр — поиск, список карточек и текст выбранной заметки. Пустые Folders и
Smart Groups не рисуются. Повторный клик по кнопке скрывает раздел. Пункт
Snippets есть и в меню View.

Заметка — название и произвольный текст: код, команда или запись. Библиотека
лежит в `snippets.json` в корне открытого проекта и не общая для всех проектов.
Файл создаётся при первом открытии раздела. Отсутствие файла — пустая библиотека.
Пустой или повреждённый файл не перезаписывается: правка отключается, ошибка
уходит в ProblemLog. Запись идёт worker-ом, атомарно, с debounce 250 мс. На диске
сохраняется порядок создания; на экране список идёт по времени изменения.
Своё сохранение и несохранённая правка не затираются внешней перезагрузкой.
Удаление файла с диска очищает память и не создаёт файл заново, пока пользователь
сам не изменит библиотеку.

Теги можно добавлять и снимать у готовой заметки; одинаковые теги без учёта
регистра не дублируются. Слева тег — строка со счётчиком. Заметку можно править
и удалять: контекстное меню карточки или Delete, когда курсор не в поле ввода.
Практические пределы файла: 5000 заметок, заголовок 500 символов, текст 256 КиБ,
до 64 тегов по 40 символов. Сегодняшняя дата показывается как часы, более ранняя —
как ДД.ММ.ГГГГ.

В шапке редактора справа три кнопки: копировать, назначить теги и открыть в
консоли. Копирование кладёт текст в буфер обмена. «Открыть в консоли» копирует
текст и вставляет его в открытый терминал или создаёт новый терминал в корне
проекта, если живой сессии нет. Enter не нажимается. Многострочный текст уходит
только в режиме bracketed paste и ждёт его до двух секунд; по таймауту остаётся
в буфере. Управляющие символы, кроме табуляции и перевода строки, и пакет больше
64 КиБ в терминал не отправляются. Пока открыт Snippets, Undo, Redo и поиск не
трогают буфер кода; копирование без фокуса в поле копирует тело заметки.

Версия workspace/lock — 0.1.43, следующая — 0.1.44.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 356 passed,
0 failed, 10 прежних optional ignored. Новые тесты покрывают запись и порядок
файла, отказ перезаписывать пустой и повреждённый файл, отбраковку дублей и
пределов, теги без учёта регистра, сортировку и поиск, даты, копирование тела,
создание файла при открытии и повторную загрузку после удаления, а также вставку
в терминал без лишнего Enter. Headless-тест полосы проверяет, что слот после
Docker открывает Snippets, следующий — Tools, а следующий слот пуст. Ручной
native GUI и Windows build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
14647905e1bbe0fb79f86aa43a074f99466f30a76784323657e1c6a8fad8f631 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.2 GiB (29483 файла).
Удалены untracked Finder .DS_Store. Финальный размер проекта — 392 MiB,
binaries — 110 MiB. Исходники, история и готовые бинарники/legal materials
сохранены; после очистки сборок не было.

### GUI для OpenAPI JSON (v0.1.44, 2026-11-06)

`openapi.json` и `swagger.json` (также `.yaml`/`.yml` и имена с `.openapi.` /
`.swagger.`) в дереве и на вкладке получают зелёную круговую метку. Остальной
JSON, включая коллекции Postman, остаётся со значком `{}`. YAML с таким именем
получает только метку.

Документ OpenAPI 3.x или Swagger 2.0 в JSON открывается как обычный редактор.
Над ним переключатель JSON | GUI. GUI не пишет файл: можно выбрать сервер,
заполнить авторизацию и отправить запрос. apiKey уходит в header, query или
cookie; HTTP Basic и Bearer поддерживаются. OAuth2 и OpenID Connect принимают
готовый токен и отправляют его как Bearer. Пустой `security` операции отменяет
общую схему. Черновики Try it out, выбранный сервер и токены живут в памяти
сессии и не сохраняются на диск. Сломанный JSON, в котором ещё узнаётся
спецификация, оставляет переключатель и показывает ошибку разбора.

Операции сгруппированы по тегам. В карточке — метод, путь, параметры, пример и
схема тела, таблица ответов и результат вызова: код, заголовки, тело, curl и
время. Локальные `$ref` раскрываются; цикл не роняет документ. Порядок полей
примера совпадает с файлом (`serde_json` `preserve_order`). Запрос идёт вне
потока интерфейса, timeout 30 с, тело ответа до 512 КиБ. Относительный URL
сервера перед отправкой нужно заменить на абсолютный `http://` или `https://`.

Версия workspace/lock — 0.1.44, следующая — 0.1.45.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 367 passed,
0 failed, 10 прежних optional ignored. Новые тесты покрывают разбор OpenAPI 3
и Swagger 2, порядок полей примера, path/query и заголовок Token, пустой
security операции, Basic/Bearer и переменные сервера, относительный URL,
обязательный path-параметр, локальный HTTP-вызов и переключатель JSON | GUI
в headless egui. Иконка проверяется по имени файла: openapi.json и swagger.yaml
получают метку, коллекция Postman — нет. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
6d95f61d3f41fd3481c14bdc8454b998196df0826a3fa82cd58c7abe728f2280 и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 8.2 GiB (25182 файла).
Удалён untracked Finder .DS_Store в binaries/macos-arm64, из-за которого
--check видел лишний файл. Финальный размер проекта — 392 MiB,
binaries — 110 MiB. Исходники, история и готовые актуальные бинарники со всеми
legal materials сохранены; после очистки сборок не было.

### HTTP Requests (v0.1.45, 2026-11-06)

В верхней группе левой полосы, между Snippets и Tools, появилась кнопка HTTP
Requests с иконкой двух стрелок. Раздел занимает то же окно, что и Snippets:
полоса кнопок остаётся, левая колонка показывает Library (All requests и
Untagged) и Tags, центр — поиск, список карточек и построитель выбранного
запроса. Повторный клик по кнопке скрывает раздел. Пункт HTTP Requests есть и
в меню View. Ширина колонки хранится в `layout.json` полем `http_requests`
(по умолчанию 0.18); старый layout без поля открывается с этим значением.

Запрос — имя, метод, URL, query, заголовки, тело и теги. Библиотека лежит в
`http_requests.json` в корне открытого проекта. Файл создаётся при первом
открытии раздела. Отсутствие файла — пустая библиотека. Пустой или повреждённый
файл не перезаписывается: правка отключается, ошибка уходит в ProblemLog.
Запись идёт worker-ом, атомарно, с debounce 250 мс. На диске сохраняется порядок
создания; на экране список идёт по времени изменения. Удаление файла с диска
очищает память и не создаёт файл заново, пока пользователь сам не изменит
библиотеку.

Методы: GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS, TRACE. Query, Headers и
Body — вкладки построителя. Query — пары имя/значение и уходит также у запроса
с телом. URL с вопросительным знаком раскладывается в эти строки. Тело: нет
тела, JSON, текст, form URL encoded или multipart form-data. Снятый флажок
остаётся в файле и не попадает ни в вызов, ни в cURL. Под построителем окно
cURL. Правка построителя переписывает команду в том же кадре. Правка команды
обновляет метод, URL, query, заголовки и тело и сохраняет id, имя и теги.
Неразобранная команда показывает ошибку и не очищает построитель. Send и
Cmd+Enter отправляют запрос. Ответ показывает код, время, заголовки и тело
до 512 КиБ. Вызов идёт вне потока интерфейса: timeout 30 с, connect 10 с,
не больше 10 редиректов, user-agent `nozhik`. Нужен абсолютный `http://` или
`https://`.

Теги можно добавлять и снимать у выбранного запроса; одинаковые теги без учёта
регистра не дублируются. Удаление — контекстное меню карточки или Delete, когда
курсор не в поле ввода. Пока открыт раздел, Undo, Redo и поиск не трогают буфер
кода; копирование без фокуса в поле копирует текст cURL. Пределы: 2000 запросов,
имя 200 символов, URL 8 КиБ, тело 256 КиБ, 100 полей по 8192 символа, 64 тега
по 40 символов.

Кнопка To OpenAPI у выбранного запроса дописывает операцию в открытый OpenAPI
JSON или в `openapi.json` в корне проекта. Если спецификаций несколько, кнопка
открывает меню. Пустой файл становится OpenAPI 3.0.3. Соседние методы и ответы
сохраняются. Запись идёт через буфер открытого документа или через файловый
worker, не с потока интерфейса. В раскрытой карточке операции OpenAPI кнопка
To HTTP Requests переносит метод, путь, параметры, заголовки и тело в этот
раздел и открывает его. Импорт до загрузки библиотеки ждёт файл и не теряется.

Версия workspace/lock — 0.1.45, следующая — 0.1.46.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 378 passed,
0 failed, 10 прежних optional ignored. Новые тесты покрывают круговой обмен
построителя и cURL для GET, JSON, form и multipart, кавычки и переносы строк,
сохранение имени и тегов при правке cURL, отказ перезаписывать повреждённый
файл, импорт и создание файла при открытии, запись OpenAPI 3 и Swagger 2 без
потери соседнего метода, локальный POST и перенос операции OpenAPI в запрос.
Headless-тест полосы проверяет, что слот после Snippets открывает HTTP Requests,
следующий — Tools, а следующий слот пуст. Кнопка To HTTP Requests видна на
раскрытой операции. Ручной native GUI и Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
be0bc174f36e9aacc591d5a35ddae068296e2d240b8d0ebacd6bd1098502d1ac и SHA-256
файлов. Упаковщик выполнил codesign --verify --deep --strict. Windows-комплект
не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.3 GiB (26399 файлов).
Удалены untracked Finder .DS_Store, включая файл в binaries/macos-arm64, из-за
которого --check видел лишний payload. Финальный размер проекта — 394 MiB,
binaries — 110 MiB. Исходники, история и готовые актуальные бинарники со всеми
legal materials сохранены; после очистки сборок не было.

### Цветовые схемы (v0.1.46, 2026-11-06)

Редактор, Git, Snippets, HTTP Requests и остальные окна читают одну палитру из
`crates/ui/assets/color-schemes.json`. Стандарт — прежняя New UI Dark (`dark`).
Рядом New UI Light (`light`) и Catppuccin Mocha (`catppuccin`): среди живых
схем редакторов чаще всего встречаются наследник VS Code Dark+, One Dark,
GitHub, Dracula и Catppuccin; третий встроенный вариант — Catppuccin, потому
что он заметно отличается от серого New UI и быстро набирает установки.
Settings → General переключает схему сразу. `settings.toml` принимает только
эти три id. Подсветка по умолчанию следует схеме; для типа файла остаются
Muted и Plain Text.

Токены покрывают хром, кнопки, выделение, hover, disabled, primary, diff,
severity, методы HTTP, метку OpenAPI, синтаксис и базовую ANSI-палитру.
Радиус 8 и ширина разделителя 5 общие. Значок проекта, 256-цветный куб
терминала, образец CSS и произвольный ANSI RGB остаются цветами содержимого.
Правило для новых элементов записано в coding-standards.md: брать токен, а
если роли нет — добавить её во все схемы.

Версия workspace/lock — 0.1.46, следующая — 0.1.47.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 383 passed,
0 failed, 10 прежних optional ignored. Headless-тесты проверяют, что JSON
совпадает со списком Settings, тёмная схема сохраняет прежние цвета редактора,
светлая и Catppuccin держат контраст текста, а неизвестное поле и битый цвет
отклоняются. Ручной native GUI и Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
1a712e75e12f4b9c7cbd683014c4ba860b31fca8434cc85de0c138e8da51bd29 и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 7.4 GiB (24399 файлов).
Удалены Finder .DS_Store, включая файл в binaries/macos-arm64, из-за которого
--check видел лишний payload. Финальный размер проекта — 394 MiB,
binaries — 110 MiB. Исходники, история и готовые актуальные бинарники со всеми
legal materials сохранены; после очистки сборок не было.

### Поля форм (v0.1.47, 2026-11-09)

Однострочные поля идут через `ui::form::singleline`: высота совпадает с кнопкой,
текст выровнен по вертикали. `form::begin` добавляет 2 логических пикселя между
строками формы и не меняет списки. `interact_size` равен высоте кнопки, поэтому
выпадающий список страниц таблицы стоит на одной линии с кнопками. Колонки
Snippets и HTTP Requests имеют боковой отступ 8. Вкладки Git Changes и History
стоят вплотную, без пустой полосы под ними. Обычный шрифт интерфейса берёт
недостающие символы (`●`, `→`) из Noto Sans Symbols 2 и JetBrains Mono.

Версия workspace/lock — 0.1.47, следующая — 0.1.48.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 385 passed,
0 failed, 10 прежних optional ignored. Headless-тест сверяет высоту однострочного
поля и центр его текста с кнопкой, а выпадающий список страниц — с той же линией.
Пропорциональный шрифт покрывает ● → ▶ ▼ · ✓. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
45f0f789e09e06b338058568054875ff0a821aa44ed1b6695f7c838ead0711a3 и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 8.2 GiB (25169 файлов).
Удалены Finder .DS_Store, включая файл в binaries/macos-arm64, из-за которого
--check видел лишний payload. Финальный размер проекта — 395 MiB,
binaries — 110 MiB. Исходники, история и готовые актуальные бинарники со всеми
legal materials сохранены; после очистки сборок не было.

### Строка поиска (v0.1.48, 2026-11-09)

Кнопка «+» в Snippets и HTTP Requests больше не выходит за колонку. Ширина поля
Search считается по рамке, а не по тексту внутри неё, поэтому кнопка остаётся
внутри строки. Справа остаётся тот же отступ 8, что и слева у поля.

Версия workspace/lock — 0.1.48, следующая — 0.1.49.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 386 passed,
0 failed, 10 прежних optional ignored. Headless-тест держит правый край кнопки
«+» на краю отведённой строки. Ручной native GUI и Windows build/Clippy не
запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
12a116711c08236c9a27114999e81c97c0f96871e4be3969195293fe5d20893d и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.1 GiB (29170 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 395 MiB, binaries — 110 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### PHP-линтеры в Docker (v0.1.49, 2026-11-09)

Кнопка с жучком запускает настроенные PHP-линтеры текущего проекта внутри
Docker-контейнера: `PHPStan`, `PHPCS`, `Psalm` и `PHPMD`. Запуск только по
нажатию. Команда в терминале контейнера запоминает линтер, если его ещё нет
в Settings → Linters; уже сохранённая запись не меняется. JSON разбирается
в общие диагностики, пути контейнера переводятся на хост, Problems показывает
вкладки, редактор отмечает строки, панель агента вставляет список в черновик.
Ненулевой код при валидном JSON — это отчёт. Невалидный JSON и ошибка
конфигурации остаются в Problems.

Версия workspace/lock — 0.1.49, следующая — 0.1.50.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings,
затем повторный Clippy `nozhik-ui` после ограничения компактного списка
категорий; cargo test --workspace --all-features --offline -- --test-threads=8 —
395 passed, 0 failed, 10 прежних optional ignored. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
f4e343453a93414bcaddf62b53a730d67f64090c2bc7beb13811b4e493bff97e и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.5 GiB (26628 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 396 MiB, binaries — 111 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### PHPUnit в консоли Docker (v0.1.50, 2026-11-09)

Кнопка запуска открывает консоль настроенного контейнера и выполняет PHPUnit.
Вывод остаётся в терминале. Settings → Linters и Settings → Tests переключают
языки, включённые в Languages: у PHP прежние линтеры и форма PHPUnit, у
остальных языков страницы пустые. Команда `phpunit` в терминале контейнера
запоминается, если настройки ещё нет.

Версия workspace/lock — 0.1.50, следующая — 0.1.51.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features --offline -- --test-threads=8 — 400 passed,
0 failed, 10 прежних optional ignored. Ручной native GUI и Windows build/Clippy не
запускались. Commit и push не выполнялись.

### PHP override completion и закрытие usages (v0.1.51, 2026-11-09)

Окно использований закрывается кликом снаружи рамки. Крестик совпадает по
размеру с соседней кнопкой отдельного окна. В объявлении метода PHP предлагаются
public и protected методы родителя, интерфейса и трейта, вместе с параметрами
и типом возврата. Private и final не предлагаются.

Версия workspace/lock — 0.1.51, следующая — 0.1.52.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 403 passed,
0 failed, 10 прежних optional ignored. Ручной native GUI и Windows build/Clippy не
запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
e1b1375b1c90abe189ab2b1e40f456c13d5cbc85b26030693765c89f1d02a4e8 и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.2 GiB (27548 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 397 MiB, binaries — 111 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Поиск по файлу и по проекту (v0.1.52, 2026-11-09)

Ctrl/Cmd+F открывает строку поиска и ставит курсор в поле Find. Search Everywhere
больше не имеет вкладки Symbols. Classes показывает объявления классов PHP и
селекторы CSS. Files и Text ищут по всему дереву проекта, включая файлы вне
языкового индекса и пути из `.gitignore`: сначала поддерживаемые расширения,
затем остальные, со spinner, пока чтение не закончено. Список результатов
занимает большую часть окна. Повтор запроса читает кэш в памяти; изменение файла,
смена проекта и выход этот кэш сбрасывают. Двоичные файлы и файлы больше 2 МиБ
пропускаются.

Версия workspace/lock — 0.1.52, следующая — 0.1.53.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 412 passed,
0 failed, 10 прежних optional ignored. Headless-тесты проверяют фокус Find,
переход к совпадению, все вкладки, индексированные и обычные файлы, кэш,
пропуск двоичных файлов и каталога `.git`. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
802ab8e4e7f475fc9b73031a2464901fa9f778083a25ce013cc9029e5c30ec3e и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.3 GiB (28861 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 398 MiB, binaries — 111 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Markdown Text / GUI (v0.1.53, 2026-11-09)

Файлы `.md` и `.markdown` открываются как исходник с переключателем Text / GUI.
Text подсвечивает маркеры и рисует курсив, зачёркивание и фон встроенного кода.
GUI показывает оформленный документ: заголовки, жирный шрифт, таблицы, списки,
цитаты и ссылки. Картинки не скачиваются. Превью ограничено 1 МиБ. Reformat Code
для Markdown не запускается. Уже сохранённый непустой список языков получает
Markdown один раз; пустой список остаётся пустым.

Версия workspace/lock — 0.1.53, следующая — 0.1.54.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 415 passed,
0 failed, 10 прежних optional ignored. Headless-тесты проверяют подсветку
исходника и переключение Text/GUI. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
54822fe9fafa7ba5cf0a33fa146b65aa6a91cdb6e6400f08339258a7d4aee78f и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 10.3 GiB (29565 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 400 MiB, binaries — 112 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Таблицы Markdown (v0.1.54, 2026-11-10)

GUI рисует таблицу GitHub общей сеткой. Ширина колонки берётся из текста этой
колонки: не уже самого широкого слова или фрагмента в обратных кавычках и,
если строка влезает в панель, равна самой длинной неперенесённой строке.
Когда сумма шире панели, колонки с переносом сжимаются до этого минимума.
Если минимум не входит, таблица едет по горизонтали. Ячейки одной строки
одной высоты, текст сверху, перенос по словам. Код в обратных кавычках не
рвётся по буквам. Выравнивание `---`, `:---`, `:---:` и `---:` задаёт край
строки в колонке.

Версия workspace/lock — 0.1.54, следующая — 0.1.55.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 423 passed,
0 failed, 10 прежних optional ignored. Headless-тесты проверяют целые
код-фрагменты, общую высоту строки, сетку и выравнивание колонок. Ручной
native GUI и Windows build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
a17280405091f1e169006083d13397f913d5f77e5b4245ebf148130d2af73434 и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 8.5 GiB (25439 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 402 MiB, binaries — 113 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Встроенный код и маркеры Markdown (v0.1.55, 2026-11-10)

В абзаце, заголовке и пункте списка фрагмент в обратных кавычках остаётся в
строке: моноширинный шрифт и фон `chip`, перенос вместе с соседними словами.
Плотный список не оборачивает пункт в абзац, поэтому текст и код пункта
собираются в один galley. Маркер маркированного списка — диск по центру
первой строки пункта. Номер нумерованного списка тоже стоит по центру первой
строки.

Версия workspace/lock — 0.1.55, следующая — 0.1.56.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 424 passed,
0 failed, 10 прежних optional ignored. Headless-тест держит `LICENSE`,
`settings/*.php` и `SECURITY.md` в строке рядом с текстом и проверяет центр
маркера на первой строке многострочного пункта. Ручной native GUI и Windows
build/Clippy не запускались. Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
84aa881a7de3ec8a303cbfc69c49f1f24f6d2ac7a7633198d8dd01c57db1e58e и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 8.6 GiB (25682 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 402 MiB, binaries — 113 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Ссылки и размер шрифта Markdown (v0.1.56, 2026-11-10)

В GUI ссылка на файл открывает его в редакторе. Относительный путь считается
от каталога текущего Markdown; `file://` тоже становится путём. `http`,
`https` и `mailto` открываются в браузере системы. Якорь и неизвестная схема
не открываются. Ссылка в ячейке таблицы следует тому же правилу.

Над GUI три кнопки A, от меньшей к большей. По умолчанию включена маленькая.
Средняя и большая умножают шрифты и отступы превью на 1.35 и 1.7.

Версия workspace/lock — 0.1.56, следующая — 0.1.57.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 428 passed,
0 failed, 10 прежних optional ignored. Headless-тест кликает относительную
ссылку и ссылку в ячейке таблицы и дожидается открытия временных файлов.
Клик по средней и большой A увеличивает высоту строки, промежуток абзацев
и радиус маркера. Ручной native GUI и Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
a81ca14527bdd9bdeab9bd9d6585820a51df6455c849715eefa2060772286da0 и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.5 GiB (29730 файлов).
Finder .DS_Store не было. Финальный размер проекта — 402 MiB, binaries — 113 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Редактор JavaScript (v0.1.57, 2026-11-10)

Файлы `.js`, `.jsx`, `.mjs` и `.cjs` получают подсветку tree-sitter-javascript,
синтаксические ошибки, индекс классов и методов и подсказку стандартных
объектов ECMAScript и `console`. TypeScript не распознаётся. Линтера, запуска
тестов и форматтера JavaScript нет: команда форматирования отвечает
`No JavaScript formatter`. Файл остаётся на локальном потоке синтаксиса и не
синхронизируется с PHP LSP.

Индекс пишется в `index_js_v6.json` и не удаляет старый PHP-кэш. Снимки PHP и
JavaScript одновременно остаются в очереди. Каталог языков версии 3 один раз
добавляет JavaScript в непустой сохранённый список. Пустой список не меняется.

Версия workspace/lock — 0.1.57, следующая — 0.1.58.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 438 passed,
0 failed, 10 прежних optional ignored. Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
613390dd42dfd0781d72747e5550bb1124ec02cd3f7ec3d01c57e789cecc022f и контрольные
суммы файлов. Упаковщик выполнил codesign --verify --deep --strict.
Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.3 GiB (29418 файлов).
Удалены Finder .DS_Store. Финальный размер проекта — 403 MiB, binaries — 113 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Отступ Enter и DOM-подсказки JavaScript (v0.1.58, 2026-11-10)

Enter в PHP, CSS, JSON и JavaScript ставит каретку по одному правилу вложенности.
Каждая незакрытая `{` или `[` добавляет один уровень профиля. Незакрытые `(`
добавляют один уровень только если в строке нет блока, поэтому
`links.forEach(function (link) {` и `(function () {` сдвигают курсор на один
шаг, как `if (...) {` в PHP. Закрывающая строка оставляет свой отступ.
Профиль `javascript_default.json` подключён к `.js`, `.jsx`, `.mjs` и `.cjs`:
шаг — 4 пробела, строка из табов остаётся на табах. Markdown получил профиль,
который копирует отступ строки и не добавляет уровень из-за скобок.
Reformat Code для JavaScript и Markdown по-прежнему сообщает, что форматтера нет.

`document.`, `window.` и связанные стандартные объекты предлагают публичные
члены. `document.querySelectorAll` типизируется как `NodeList`. Параметр
`forEach` и переменная `for...of` получают тип элемента, поэтому у `link` есть
`addEventListener`. Каталог не покрывает всю спецификацию DOM.

Версия workspace/lock — 0.1.58, следующая — 0.1.59.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 442 passed,
0 failed, 10 прежних optional ignored. Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
8e25cabced8591a4edcc882ec16d6869ae9d65bbd7fffcb19645a1c5ec87d887 и контрольные
суммы файлов. Упаковщик выполнил codesign. Windows-комплект не обновлялся.

После проверки готовых бинарников cargo clean удалил 9.2 GiB (27861 файл).
Удалены Finder .DS_Store вне `.git`. Финальный размер проекта — 403 MiB,
binaries — 113 MiB. Исходники, история и готовые актуальные бинарники со всеми
legal materials сохранены; после очистки сборок не было.

### Каталог JavaScript для страницы (v0.1.59, 2026-11-10)

`document` и `querySelectorAll` уже были в каталоге. Не хватало типов, которые
нужны в обычном скрипте страницы. `querySelector`, `getElementById` и
`createElement` теперь дают `HTMLElement`: у элемента есть `style`, `dataset`,
`value`, `href`, `src` и остальные поля, которые вводят у ссылок, полей и
холста. Элемент коллекции `NodeList` и `HTMLCollection` тоже `HTMLElement`.
Слушатель `addEventListener('click')` получает `MouseEvent`, `keydown` —
`KeyboardEvent`, неизвестное имя — `Event`. `await fetch(...)` и
`fetch(...).then` дают `Response`, `await response.text()` — `String`.

В каталог добавлены сеть и адрес (`URL`, `URLSearchParams`, `FormData`,
`Headers`, `Request`, `Response`, `AbortController`), файлы, `TextEncoder`,
`DOMParser`, `XMLHttpRequest`, `WebSocket`, наблюдатели, `History`, `Crypto`,
`Performance`, выделение, буфер обмена, IndexedDB, форматеры `Intl`, холст 2D,
`Image`/`Audio`, `Range` и события указателя, колеса, перетаскивания и
сообщений. `globalThis` и `self` ведут себя как `window`. Частые свойства
`element.style` перечислены. Ключи `data-*` и остальные имена CSS остаются
динамическими. WebGL, WebRTC, Web Audio и отдельный интерфейс каждого тега не
входят. Список подсказки по-прежнему показывает 100 строк: методы и поля
страницы идут раньше длинного списка атрибутов.

Кэш индекса сменился на `index_js_v7.json`, потому что выведенные типы
изменились. Форматтера, линтера, запуска тестов и TypeScript по-прежнему нет.

Версия workspace/lock — 0.1.59, следующая — 0.1.60.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 446 passed,
0 failed, 10 прежних optional ignored. Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
143d5e5fcf14a548498ee9c2afddff0f0c4edcce3b1d6e77ca4129486d99c311 и контрольные
суммы файлов. Упаковщик заменил существующую подпись. Windows-комплект не
обновлялся.

После проверки готовых бинарников cargo clean удалил 9.3 GiB (30031 файл).
Finder .DS_Store вне `.git` не было. Финальный размер проекта — 403 MiB,
binaries — 114 MiB. Исходники, история и готовые актуальные бинарники со всеми
legal materials сохранены; после очистки сборок не было.

### Ключи объекта JavaScript (v0.1.60, 2026-11-10)

Литерал `{ owner_id, contact_type, source }` больше не остаётся безымянным
`Object` без собственных полей. Индекс запоминает ключи, которые можно
прочитать через точку: идентификаторы, строковые ключи, короткие свойства и
методы. `payload.` открывает эти ключи, а следом методы `Object.prototype`
(`toString`, `hasOwnProperty`). `parseInt(...)` даёт ключу тип `Number`,
вложенный литерал сохраняет свои ключи, копия переменной получает тот же тип.
Вычисляемые ключи, spread и динамические `data-*` по-прежнему не перечисляются.
Пустой `{}` остаётся `Object` и показывает методы прототипа. Ключи литерала не
приглушаются как неиспользуемые свойства класса.

Кэш индекса — `index_js_v8.json`. Форматтера, линтера, запуска тестов и
TypeScript по-прежнему нет.

Версия workspace/lock — 0.1.60, следующая — 0.1.61.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 449 passed,
0 failed, 10 прежних optional ignored. Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
f3876e76b4b016b008b427af1f6825e78c70481e0db506d5f25164ab71af7424 и контрольные
суммы файлов. Упаковщик заменил существующую подпись. Windows-комплект не
обновлялся.

После проверки готовых бинарников cargo clean удалил 10.8 GiB (32402 файла).
Случайные Finder .DS_Store вне `.git` удалены; повторный --check подтвердил
актуальность macOS ARM64. Финальный размер проекта — 403 MiB, binaries — 114 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

### Окно подсказки JavaScript после точки (v0.1.61, 2026-11-10)

Точка в JavaScript больше не теряет окно на следующем кадре. Редактор
пересчитывает список по всему файлу, но приёмником считается выражение
непосредственно перед `.` или `?.`. `document.`, `console.`, `Math.` и
`new URL(...)` внутри функции показывают свои методы. `payload.` показывает
собственные ключи и методы `Object.prototype`. Более ранний код функции не
подменяет имя объекта.

Кэш индекса остаётся `index_js_v8.json`. Форматтера, линтера, запуска тестов и
TypeScript по-прежнему нет.

Версия workspace/lock — 0.1.61, следующая — 0.1.62.

Проверки: native fmt; Clippy workspace/all-targets/all-features с -D warnings;
cargo test --workspace --all-features -- --test-threads=8 — 452 passed,
0 failed, 10 прежних optional ignored. Windows build/Clippy не запускались.
Commit и push не выполнялись.

Workspace release build прошёл. Штатный update-binaries.py обновил macOS ARM64
executable и app archive; --check подтвердил source fingerprint
a7982e8e7e77e9580eb9eb6cdf8bf3c540527c7cd5d6c3177c69dd857a6756a3 и контрольные
суммы файлов. Упаковщик заменил существующую подпись. Windows-комплект не
обновлялся.

После проверки готовых бинарников cargo clean удалил 13.6 GiB (39562 файла).
Случайный Finder .DS_Store вне `.git` удалён; повторный --check подтвердил
актуальность macOS ARM64. Финальный размер проекта — 403 MiB, binaries — 114 MiB.
Исходники, история и готовые актуальные бинарники со всеми legal materials
сохранены; после очистки сборок не было.

