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
| 4 | Tree-sitter, подсветка, синтаксические индексы | Не начат |
| 5 | LSP, диагностика, completion, переходы и рефакторинг | Не начат |
| 6 | БД: подключения, схема, SQL-консоль, результаты | Не начат |
| 7 | Terminal, task runner, запуск и отладка | Не начат; Git вынесен в 3.4 |
| 8 | Плагины, темы, keymap, API расширений | Не начат |
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
