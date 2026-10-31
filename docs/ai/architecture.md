# Архитектура Nozhik — Phase 9 / v0.1.0-RC1

## Границы крейтов

| Крейт | Ответственность | Внутренние зависимости |
| --- | --- | --- |
| nozhik-core | Значимые типы и их инварианты | Нет |
| nozhik-event-bus | Системные события и транспорт pub/sub | core |
| nozhik-buffer | Rope, координаты, курсоры, история, поиск | core, event_bus |
| nozhik-config | Схема, валидация, хранение и доступ к настройкам | core, event_bus |
| nozhik-fs-model | lazy tree, ignore, file index, watcher, I/O worker | event_bus, vcs (read-only membership) |
| nozhik-vcs | Git repository/index, status/diff/history, branches/remotes, serial worker | git2, thiserror |
| nozhik-plugin-host | Manifest validation, isolated process lifecycle, action/keymap registries | event_bus, tokio, serde/serde_json, semver, directories |
| nozhik-language-support | PHP incremental syntax, stdio LSP client, workers, UTF-16 conversion | tree-sitter/php, lsp-types, tokio, ropey, serde_json, thiserror |
| nozhik-terminal | Native PTY, VT100 grid/scrollback, background sessions and lifecycle | portable-pty, vt100/vte, parking_lot, nix (Unix), thiserror |
| nozhik-remote-ssh | SSHv2, protected credentials, host trust, SFTP, loopback forwarding | russh, russh-sftp, keyring, zeroize, tokio, serde/serde_json, directories, tempfile |
| nozhik-database | SQL pools, typed results, catalog metadata, background actor and owned SSH tunnels | sqlx, tokio/tokio-util, remote_ssh, keyring, serde/serde_json, tempfile |
| nozhik-ai-completion | Local Ollama FIM, cancellable generation, model inspection/download | reqwest, tokio, serde/serde_json, thiserror |
| nozhik-ui | UI shell, documents, project/Git integration, editor view, search | core, buffer, config, event_bus, fs_model, vcs, plugin_host, language_support, terminal, remote_ssh, database |
| app | Инициализация, жизненный цикл, eframe | ui, config, event_bus, ai_completion |

`app/assets` хранит исходную PNG-иконку (`source.png`, копия `docs/img/logo.png`)
и готовые PNG/ICO/ICNS. Стартовое окно использует тот же `window-icon.png`,
что и viewport icon. Composition root
декодирует встроенный PNG через eframe и задаёт viewport icon. Windows build.rs
встраивает ICO через SDK rc.exe (MSVC) или MinGW windres (GNU); runtime-зависимостей
не добавлено. scripts/update-binaries.py дополнительно упаковывает macOS
Nozhik.app.zip с executable, Info.plist и ICNS для Finder/Dock; архив и отдельный
CLI executable входят в SHA-256 metadata. Пакет получает локальную ad-hoc подпись
для целостности ресурсов; Developer ID и notarization пока отсутствуют.

Папка `crates/core` имеет имя пакета `nozhik-core`, чтобы не конфликтовать
с системным Rust crate `core`. Обратные зависимости и циклы запрещены.
Новые общие трейты добавляются в core при появлении реального потребителя;
абстрактного service locator и глобального singleton нет.

## Поток данных

1. `main` создаёт EventBus с ёмкостью 1024, загружает настройки стартового окна
   и запускает Tokio runtime с application-scoped Ollama lifecycle.
2. Главный поток запускает eframe::run_native (egui + Glow). `Workspace` владеет
   стартовым экраном и независимыми `NozhikUi` для открытых проектов. Каждый проект
   имеет отдельные bus/receiver, ConfigManager, документы и фоновые службы.
3. Worker канонизирует открываемую папку и загружает `.nozhik/settings.toml`.
   Отсутствующий файл использует defaults; ошибка сохраняет существующие сессии.
4. TextEditorView читает видимые Rope-строки активного проекта. Фоновые события
   будят окно через runtime bridge; неактивные проекты обрабатывают ответы,
   сохраняя диалоги до переключения в них.
5. Изменение настроек: `replace` → validate → write lock → замена → release
   lock → `ConfigChanged(All)`. SettingsDialog worker атомарно сохраняет файл
   конкретного проекта. Ollama lifecycle следует настройкам активного проекта.
6. Закрытие проекта проходит dirty/revision gate и завершает его службы.
   Закрытие окна проверяет все проекты, выходит из event loop, завершает lifecycle
   и предоставляет runtime до 1 секунды на shutdown.

## Шина событий

Транспорт — ограниченный `tokio::sync::broadcast<Arc<Event>>`. EventBus и Event
реализуют Send + Sync; клонирование bus создаёт producer, subscribe — отдельного
consumer. Payload разделяется через Arc, текст не копируется для каждого читателя.
`publish` синхронный, не ждёт потребителей и возвращает число подписчиков
на момент отправки. Ноль — нормальная ситуация; сообщение не сохраняется.
Подписка видит только будущие публикации. Ёмкость проверяется при создании;
Tokio может округлить её вверх до степени двойки.

События:

- `FileOpened(PathBuf)` — уведомление после успешного открытия, не запрос I/O.
- `BufferModified { id, range, text }` — уведомление об уже применённой замене.
  Диапазон относится к состоянию до изменения; пустой диапазон — вставка,
  пустой текст — удаление.
- `ConfigChanged(ConfigKey)` — инвалидирование кэша. Сейчас manager использует
  `All`; отдельные ключи зарезервированы для будущего точечного API.
- `ActionTriggered(String)` — уведомление об инициированном действии.

Каждая подписка должна обрабатывать `Lagged(n)` и `Closed`. При отставании
самые старые события теряются; после `Lagged` чтение можно продолжить.
Stateful-модули должны перечитать авторитетное состояние. Логгер приложения
только сообщает о потере. Шина не является журналом редактирования, очередью
гарантированных команд или источником истины. Для редактора используются ревизии буферов и API повторного получения состояния.
Legacy `BufferModified` сохранён для совместимости. Текстовое ядро публикует
`Event::Buffer(BufferEvent::Modified { buffer_id, revision, edits })`:
`edits` применяются последовательно в указанном порядке. Обычная транзакция
публикует замены справа налево; undo группы публикует обратную последовательность.
`CursorChanged` содержит канонические позиции для той же ревизии.
После потери событий нужен `TextBuffer::snapshot()`, а не replay broadcast.

## Типы координат

BufferId — u64 newtype; уникальность и запрет повторного использования должен
обеспечить будущий BufferStore. Ноль сейчас допустим. Position: line и column
с нуля; column измеряется в UTF-8 байтах внутри строки. Это не графемы,
экранные колонки или UTF-16 координаты LSP. Конверсия LSP UTF-16 принадлежит language_support.
Span — полуинтервал [start, end), порядок проверяется конструктором и serde.
Position дополнительно содержит абсолютный `byte_offset: usize`; при установке
курсоров TextBuffer проверяет согласованность всех трёх координат.
Границы текста и UTF-8 boundaries проверяет TextBuffer. Char-индексы означают
Unicode scalars; графемы, табуляция и экранная ширина относятся к следующему этапу.

## Текстовое ядро

`TextBuffer` принадлежит вызывающему коду, который выделяет уникальный `BufferId`.
Реестр файловых документов принадлежит UI; файловый I/O выполняет fs_model worker. `snapshot()` возвращает
Rope с copy-on-write, ревизию и курсоры; состояние доступно без подписки на шину.
Ревизия u64 монотонно растёт при commit, undo и redo. Движение курсоров не меняет
текстовую ревизию. Ошибочные и пустые правки не создают событий и истории.

LF и CRLF — переводы строк, одиночный CR — текст. Ropey настроен без
`unicode_lines`/`cr_lines`; байты исходного текста не нормализуются.
`Position` может указывать внутрь CRLF, но только на UTF-8 границы.
Конец файла допустим; следующий line start принадлежит новой строке, поэтому
колонка предыдущей строки не может переходить через её перевод строки.

Все диапазоны транзакции относятся к исходному тексту. Они сортируются,
проверяются и применяются к копии Rope; commit происходит только после
успешного пересчёта всех координат. Пересечения и одинаковые начала запрещены;
соседние диапазоны допустимы. Курсоры имеют правую привязку: точка внутри
замены или на её границе переходит в конец вставленного текста.
CursorSet сливает соприкасающиеся выделения и курсоры, сохраняя направление
левого выделения; при одинаковом начале побеждает входной порядок.

History хранит снимки Rope до/после группы и прямые/обратные замены для событий.
История пока не ограничена по размеру; старые версии удерживают изменённые чанки
и тексты замен. Группы задаются `EditTransaction::group`, без таймеров.
Сохранение, смена курсоров, undo/redo закрывают группу. `mark_saved()` запоминает
идентичность состояния, поэтому undo к сохранённой версии снимает dirty,
а новая ветка с другим состоянием остаётся dirty даже при совпадении содержимого.
Сохранение старого worker-снимка требует отдельной координации вызывающего кода;
`mark_saved()` относится только к текущему состоянию.

SearchEngine использует regex-cursor для многострочного Unicode Regex по Rope,
regex — только для экранирования литералов. Результаты — непересекающиеся
байтовые диапазоны; пустой шаблон возвращает границы Unicode scalars.

## Многопоточность и настройки

`Arc<RwLock<Config>>` даёт общий доступ к настройкам. Для чтения предпочтителен
короткий `snapshot()`, для записи — `replace()`. `shared()` открыт по требованию
контракта; прямое изменение через него обходит валидацию и события, поэтому
обычный код так не пишет. Poisoned lock — явная ошибка, тихого восстановления нет.
Нельзя держать guard через await, вызывать из-под guard код подписчиков или
рекурсивно захватывать lock. Асинхронный транспорт не требует lock приложения.

Файловый I/O синхронный: разрешён до запуска окна, позднее — в spawn_blocking
или выделенном потоке. save держит read lock до завершения записи, блокируя
replace; он не должен вызываться при уже удерживаемом settings guard.
Длительное сохранение может задержать writers. Публикация после освобождения
lock означает, что потребитель может увидеть более новое состояние, чем то,
которое вызвало событие. Уведомления — invalidation, не поток точных diff.

## Политика файлов

Расширение определяет TOML или JSON. Отсутствие файла — defaults без записи;
отсутствующие поля — serde defaults. Остальные ошибки не маскируются.
reload оставляет старое состояние при ошибке чтения/валидации.
replace меняет только память, save сохраняет явно. Транзакции memory+disk нет.
Сохранение использует NamedTempFile в том же каталоге, sync_all файла и persist
поверх назначения. Это защищает от частично записанного основного файла;
полная crash durability каталога не гарантируется. Блокировки между процессами
и слияния внешних правок нет: последний успешный save побеждает.

## Следующие архитектурные решения

UI выделяет BufferId монотонно без переиспользования и хранит документы по canonical path.
Сохранение передаёт COW snapshot и revision; успешное завершение снимает dirty
только при совпадении текущей revision. Версии документов и snapshot/resync реализованы в buffer.
UI проверяет broadcast без блокировки в update; attach_runtime запускает
ожидающий bus/watch bridge и будит окно по событию без периодического repaint. Файловый worker и watcher также
будят event loop через зарегистрированный callback `Context::request_repaint()`. UI не вызывается из Tokio-задач.
ActionTriggered(ui.search_everywhere/ui.settings) открывает modal best-effort;
состояние документов всегда читается напрямую, не восстанавливается из событий.
Renderer — Glow/OpenGL. Plugin protocol — JSON-RPC/stdio (Phase 4);
Database pools — PostgreSQL/MySQL/SQLite через SQLx (Phase 8).


## Файловая система (Phase 3)

`fs_model` не зависит от egui или buffer: принимает Rope snapshot, возвращает
UTF-8 текст, native пути и модель дерева. `FileSystem` обрабатывает команды
последовательно на отдельном потоке, `ProjectWatcher` публикует invalidation
через EventBus после debounce 150 мс. UI владеет `TextBuffer`, lazy-состоянием
раскрытых каталогов и search token; никакой дисковый I/O не выполняется в update.

`Event::File(FileEvent::Changed(PathBuf))` включает create/remove/rename/modify
и собственные сохранения. `FileEvent::Error(String)` означает неполноту наблюдения.
На Changed и Lagged UI запрашивает Refresh индекса/загруженных каталогов;
текст открытых документов остаётся в памяти. Надёжные Read/Save/LoadDir/Search
используют mpsc команды/ответы, а не broadcast. Подробности и ограничения:
[Phase 3](roadmap-progress.md#phase-3--файловая-система-и-project-model).

## VCS (Phase 3.4)

`nozhik-vcs` использует vendored libgit2/OpenSSL через git2, не зависит от UI,
буфера, runtime или EventBus. `Repository` создаётся внутри одного worker и не
передаётся между потоками. UI отправляет надёжные mpsc задачи, получает owned
снимки и проверяет generation/request ID. `VcsPanel` отвечает за выбор файла,
режим Changes/History, commit form и branch/remote controls; `DiffView` рендерит
видимые строки unified patch. History содержит полноширинные строки коммитов,
отдельную изменяемую по ширине колонку файлов и diff справа. Выделение diff
хранит пары row/Unicode-scalar offset, копирует только текст без gutter/маркеров
через Cmd/Ctrl+C, контекстное меню или native Edit → Copy. Drag выделяет текст;
колесо и обе полосы прокрутки остаются независимыми. Ширина patch измеряется
при смене файла/шрифта, чтобы горизонтальный scrollbar не зависел от видимых строк.
Результаты worker будят egui callback-ом.

`FileEvent::Changed`, broadcast lag и успешный Save инвалидируют Git status с
200 ms debounce. Фоновый опрос раз в 3 секунды покрывает `.git` и внешние команды,
даже когда открыта другая IDE-панель. Changed-файлы Git определяет самостоятельно,
поэтому ignore-политика fs_model не скрывает tracked изменения из Git status.
Чекбоксы меняют реальный index; commit не перечитывает в него worktree автоматически.
Git работает с диском; буферы редактора сохраняются в памяти после branch/pull/discard.
Полные задачи, ответы и ограничения описаны в
[roadmap Phase 3.4](roadmap-progress.md#phase-34--vcs-engine--github-desktop-ui).

### Git branch and merge UI (2026-10-29)

`vcs::merge` owns local/remote branch switching, branch metadata, branch-named
stashes, read-only merge previews and the merge/continue/abort lifecycle. All
operations stay on the serial repository worker. Remote checkout establishes a
local tracking branch and removes it if safe checkout fails. New branches keep
the current index/worktree. Leave Changes includes untracked paths in a named
stash; returning restores its index as well as working contents. A failed stash
apply retains the stash. Bring Changes uses libgit2 safe checkout.

Merge preview uses an in-memory merge index. Merge starts only from a clean
repository; a conflict-free merge commits immediately, while conflicted merges
retain MERGE_HEAD and expose owned path/remaining-conflict snapshots. Marker-free
regular same-path text resolutions are eligible for Continue; structural, binary,
missing and symlink paths require explicit index resolution. Snapshot/Continue
also detect leftover markers in already-staged files, so external git add cannot
bypass the resolution gate. Continue rechecks the disk and creates the merge
commit with both parents before cleanup_state. Abort requires UI confirmation
and restores HEAD, retaining unrelated untracked files. Marker detection is a
conservative line-prefix check, not a language parser.

`vcs_panel::controls` paints the Git chrome and owns branch/modal/file actions;
`VcsPanel` separates mutation requests from background reads. Only mutations gate
controls. It retains the current diff during async loading, labels previous
content while an uncached target loads, keeps eight bounded display patches and
checks project generation/request IDs before installing replies. Fixed row/footer
geometry and stable scroll IDs prevent status polling from moving controls.
`Layout.git` defaults to 27% and is persisted separately from the Project dock;
older layout files receive the serde default. File menus support real multi-path
staging/discard/ignore and worker-side OS open/reveal; editor-open requests return
to the existing application document loader. Git still acts on saved disk/index
contents, and existing editor buffers remain retained after repository mutations.

## Плагины и действия (Phase 4)

`PluginManager` — неблокирующий UI handle с bounded mpsc requests и watch
snapshot. Собственный worker с current-thread Tokio runtime владеет дочерними
процессами, scan, dependency graph, активацией и hot reload keymap.json.
Нативные библиотеки плагинов не загружаются в IDE; процессный протокол не требует
перекомпиляции IDE. При shutdown внешний select отменяет ожидающий RPC,
kill_on_drop защищает незавершённую инициализацию, известные процессы kill/reap.
UI подписан на watch snapshots; timer хоста публикует изменения только при
изменении keymap/diagnostics/процессов, не вызывает redraw на каждом idle tick.

ActionRegistry и KeymapRegistry — Arc/RwLock снимки; блокировки освобождаются
до событий и запросов. Встроенное действие публикует ActionTriggered; команда
расширения сначала попадает в mpsc, затем публикует best-effort уведомление.
Процесс не получает повторный вызов из broadcast; RPC имеет отдельный ID и ack.
`FileOpened` активирует языковые плагины; пропуски шины диагностируются,
повторное открытие повторяет событие. `HostSnapshot` содержит только включённые
языковые ассоциации, tool tabs и typed configuration defaults.

Граница изоляции — отдельный процесс с правами пользователя, без sandbox.
Протокол, ограничения и API: [руководство](plugin-authoring-guide.md).


## Языковая подсистема (Phase 5)

UI language state передаёт Rope/revision через LanguageService в независимые
syntax worker и async LSP actor. Tree-sitter parser/tree принадлежит документу
в worker, stdio Child — LspManager в actor. Отдельный transport reader
демультиплексирует JSON-RPC responses и notifications. LSP не использует
plugin_host NDJSON: Content-Length framing и lifecycle — отдельный протокол.
UI получает spans/diagnostics и typed response events с URI/revision/token,
проверяет актуальность и рисует только viewport. На смене проекта service
отменяется; на выходе bounded shutdown/exit завершается kill-on-drop fallback.
Полные форматы команд/событий и границы реализации:
[roadmap Phase 5](roadmap-progress.md#phase-5--tree-sitter-syntax-highlighting--lsp-client-core).


## Терминал (Phase 6)

TerminalPanel лениво создаёт TerminalManager; new SessionOptions получает cwd
проекта, а shell discovery и PTY spawn выполняются background control worker.
Master/Child не попадают в egui. Reader/parsing и writer разделены, поэтому
насыщенная запись не блокирует polling/close. VT100 (на vte) владеет normal/alternate
screens и 3000 строками истории. Reader/control публикуют Arc<TerminalGrid>;
UI читает готовый snapshot через try_lock и рисует cell-aligned monospace grid.
Отдельные lifecycle events Started/Exited/Error коррелируются TerminalSessionId.
Alt+F12 разрешается глобальным Keymap в ui.terminal; фокусированный terminal
encoder передаёт control/escape/UTF-8 bytes через bounded queue в PTY writer.
Resize панели обновляет PTY и emulator в control worker. Tab close сигнализирует
stop без ожидания, App::on_exit выполняет bounded manager shutdown.
Контракты и ограничения: [roadmap Phase 6](roadmap-progress.md#phase-6--terminal-subsystem).


## Remote Development & SSH (Phase 7)

`remote_ssh` не зависит от egui, buffer, terminal или config. Russh 0.65
содержит `russh::keys` (отдельный устаревший russh-keys не нужен); выбран
ring backend без aws-lc/libssh2. SFTP v3 — russh-sftp 3.0.1. Rust SSH stack
не добавляет C SSH-библиотек; ring использует свой нативный crypto build.

`SshProfile` сериализует только публичные настройки; SHA256 pin необязателен
и сохранён для совместимости со старыми профилями.
`load_profiles/save_profiles` валидируют поля и уникальность имён; сохранение
атомарно через tempfile. UI использует соседний с settings `ssh_hosts.json`.
Никаких паролей/фраз в профилях: `connect(profile, secret)` получает их временно.
Host key проверяется до auth; CA certificates отклоняются.
Новый UI flow: Verify (SSH handshake без auth) → TrustRequired → явное Trust →
Verified → saved OS credential либо модальный Password/Passphrase → Connect. UnknownHostKey содержит
полученный fingerprint; HostKeyChanged никогда не превращается в first-use prompt.
`trust.rs` атомарно хранит host/port → SHA256 в соседнем `ssh_known_hosts.json`;
ошибки чтения/JSON не заменяются defaults, существующий другой pin не перезаписывается.
Connect повторно проверяет тот же pin на настоящем auth-соединении, поэтому смена
ключа между предварительной проверкой и вводом пароля отклоняется. Key file I/O работает
через spawn_blocking, Agent — Unix socket / Windows OpenSSH named pipe.

`SshSession` разделяет Handle через Arc<tokio::Mutex>; lock нужен только для
создания каналов/авторизации, сами shell/SFTP/direct-tcpip работают независимо.
Connect и channel setup имеют deadline 15s, keepalive interval 20s/max 3.
Отклонённый PTY/shell/subsystem не превращается в успешный результат.
Waiter ожидает Success/Failure, пропускает WindowAdjusted/XonXoff; ранние
Data/ExtendedData помещаются в ограниченную очередь (64 KiB за фазу, до 128
сообщений). `SshShellStream::wait` сначала возвращает этот вывод, затем читает
Channel, включая Data/ExtendedData/ExitStatus/Close. Настоящий Failure указывает
PTY или shell; EOF/close/exit во время setup — отдельная ошибка. SFTP setup
также пропускает window-adjust. Общий deadline ограничивает ожидание ack.

UI remote worker владеет своим current-thread runtime и типизированными bounded
command/reply очередями ёмкостью 64; завершение отменяет runtime и все задачи.
Локальные profile/cache операции используют blocking pool / tokio fs. Repaint
вызывается после публикации ответа. Connect и операции файлов последовательны,
с общим deadline команды 35s; shell/tunnel I/O продолжается независимо. Worker
проверяет закрытие транспорта раз в секунду, удаляет SFTP и туннели, сообщает Offline.
UpsertProfile/DeleteProfile загружают актуальный список с диска, проверяют имена
и атомарно сохраняют JSON, затем закрывают затронутые сессии/каналы и туннели.
UI не теряет форму при ошибке записи. Количество forwards в форме не ограничено
одним правилом. Изменение identity/legacy pin инвалидирует привязки открытых
документов: текст остаётся, но upload запрещён до повторного открытия через SFTP.
Cache document IDs монотонны даже после удаления/переиспользования имени хоста.

Терминал получает `ExternalTransport`, оставаясь независимым от SSH/Tokio.
UI adapter переводит input/resize в bounded async очередь, а Data/Exit/Error —
в выходную. Отдельный terminal worker использует существующий Emulator и
SessionSnapshot; VT100 parsing не работает на UI. Drop/close отменяет shell task
независимо от заполненных очередей; ошибки сохраняют последний экран.

`SftpClient` лениво кэширует непосредственных детей: 128 путей на соединение,
TTL 30s, директории перед файлами, dot entries отбрасываются. Символические ссылки
не раскрываются как папки. Upload/Refresh сбрасывают listings. Чтение ограничено
16 MiB во время streaming. Путь SFTP — протокольная POSIX-строка, локальный
кэш — PathBuf. Worker назначает непрозрачные document-N имена в TempDir;
серверные имена не используются как локальные пути. UI хранит привязку
cache path → (profile name, remote path), повторное открытие сохраняет буфер.
Ctrl/Cmd+S отправляет Rope snapshot; материализация происходит на worker,
ack содержит revision, dirty снимается только при её совпадении. Ошибки не
переключают сохранение на локальный fs_model. Upload CREATE/TRUNCATE/WRITE
не атомарен: при сбое сервер может содержать частичный файл, редактор остаётся dirty.

Туннель привязан к loopback IPv4, каждому сокету соответствует direct-tcpip.
Tokio copy_bidirectional поддерживает обратное давление/half-close. JoinSet
ограничивает соединения до 64; abort/drop TunnelHandle закрывает listener и все
потоки. Неявного открытия туннелей при тесте/терминале/SFTP нет.


### Защищённые SSH credentials

`credentials.rs`: Secret (Zeroizing<String>, redacted Debug), CredentialStore
и OsCredentialStore через keyring 3.6.3. macOS — apple-native (Keychain), Windows —
windows-native (Credential Manager), Linux — sync-secret-service + crypto-rust
и vendored DBus. Mock keyring backend используется только в изолированных тестах,
не как production fallback. Native store I/O работает через spawn_blocking.
Entry identity: service dev.Nozhik.Nozhik.SSH, account — сериализованный tuple
profile name/host/port/user/auth (+ path ключа). Secret не сериализуется.

В профиле boolean save_password (default false). Read credentials выполняется
после host trust; фактический auth handshake повторно проверяет pin. Save
включается явно в credential modal для уже сохранённого профиля и выполняется
после успешной авторизации. Ошибка native store не превращается в plaintext
файл или ложный успех сохранения. Missing/rejected/locked secret возвращает
CredentialsRequired; пользователь может войти без сохранения. После успешного
входа metadata preference синхронизируется обратно в UI.

ForgetPassword/удаление профиля/смена credential identity удаляет прежний native
entry; если credential store отказывает в удалении, операция профиля завершается
ошибкой до изменения JSON. При ошибке JSON после удаления секрета профиль может
остаться с preference=true, но отсутствие entry безопасно приводит к запросу
ввода. JSON и OS store не имеют общей транзакции. Trust records хранятся отдельно
и не удаляются вместе с профилем (один сервер может использоваться несколькими).


## Database Tooling (Phase 8)

`database` независим от egui, buffer, config и event_bus. `ConnectionProfile`
содержит публичные настройки и optional SSH profile name; пароли — `Secret`
из remote_ssh, с redacted Debug и zeroize. Сериализация профилей не содержит
секретов. Атомарный JSON расположен рядом с settings в `database_connections.json`.
OS credentials используют отдельный service `dev.Nozhik.Nozhik.Database` и tuple
name/driver/host/port/user/database. Native credential/file I/O — spawn_blocking;
opt-in сохранение требует успешного теста того же профиля и пароля. JSON и OS
store не имеют общей транзакции; ошибка JSON может оставить неиспользуемый entry.

`Connection` владеет enum SQLx pool (PG/MySQL максимум 4, SQLite 1) и optional
`TunnelHandle`. Только авторизованный `SshSession` из существующего remote worker
может открыть loopback port 0 → direct-tcpip → remote DB endpoint. Новый
`Intent::Database(request_id)` проходит штатные TOFU/credential dialogs; UI передаёт
clone session в database worker, не получая russh handles. Никакого обхода host trust.

`worker::Worker` владеет отдельным current-thread Tokio runtime + LocalSet,
bounded command/reply очередями по 32, максимум 16 query tasks. Connect/Test
ограничены 45s, refresh метаданных 30s. Repaint callback будит egui. Query ID
монотонный, принадлежит вкладке; закрытие вкладки/Disconnect/Drop отменяет работу.
Раз в 5s проверяются idle server connections и tunnel lifetime; занятый пул
не считается offline. Проверка ping ограничена 2s на соединение.

`query` использует raw SQL streaming с отдельным QueryResult на statement:
ColumnInfo { name, data_type }, rows Vec<Vec<DbValue>>, execution_time_ms,
rows_affected, truncated. Сохраняется максимум 10 000 строк / 16 МиБ значений
на запрос; это лимит retained results, не максимального размера одного SQL-пакета.
CancellationToken обрывает ожидание; серверное соединение после отмены/ошибки/
лимита закрывается, SQLite использует safe SQLx progress handler, чтобы прервать
VM без потери in-memory БД. Отмена не откатывает уже завершённые autocommit statements.
Схема PG/MySQL устанавливается перед каждым запуском на той же pool connection;
переход к Default schema сбрасывает состояние предыдущей вкладки.

`DatabaseSchemaTree` → SchemaInfo → TableInfo { is_view, columns, objects } → SchemaColumn
{ name, data_type, nullable, is_primary_key }. PostgreSQL использует information_schema
с join полного constraint identity, MySQL/MariaDB — information_schema с явным
UTF-8 CAST для бинарных catalog columns, SQLite — database_list/sqlite_master/
table_info. Идентификаторы в preview/PRAGMA цитируются по диалекту, catalog filters
используют bind parameters. Tree принадлежит UI: начальный snapshot содержит только имена, затем обновляется
по ответам Describe одной таблицы. `catalog()` не читает колонки/ключи/DDL;
`describe_table()` загружает один relation, `introspect()` остаётся полным API для
проверок/явных потребителей. TableInfo.details_loaded отличает пустые placeholders.
Command::Describe запускается в bounded task с timeout 20s; Reply::Table сверяется
с request ID и активным каталогом. Ошибка видна внутри узла с Retry; ошибка
keys/DDL сохраняется в TableObjects.error и не блокирует колонки/данные.
Connect имеет отдельные пределы 45s для транспорта и 30s для имён каталога;
Test Connection проверяет только handshake/authentication. UI показывает
Connecting… и устанавливает Database Connected только после готовности имён.
SSH transport ready не считается готовностью БД.

Command::Update несёт исходный и изменённый профиль: worker заменяет ровно
ожидаемую запись, отклоняет дубликат имени/устаревшую форму, атомарно сохраняет
JSON и только после успеха отменяет старое соединение. Новые сохранённые
credentials требуют теста тех же полей/пароля; прежний credential можно сохранить
для того же endpoint. При переименовании сохранённый credential копируется в
новую identity в background blocking API, без сериализации секрета. Старые
вкладки помечаются connection_invalid: правки остаются, execution/submit/reload
запрещены даже после повторного подключения с тем же именем. Новые таблицы и
DDL не дедуплицируются с invalid-вкладками.
`TableObjects` содержит keys, foreign_keys, indexes и DDL: SQLite sqlite_master/
PRAGMA, MySQL SHOW CREATE/information_schema, PostgreSQL pg_constraint/pg_indexes/
pg_attribute. PostgreSQL DDL реконструируется из каталога (типы, defaults,
identity/generated columns, constraints, indexes); это определение таблицы,
не полный pg_dump привилегий, partition/storage settings и зависимостей.

Открытие таблицы создаёт SqlConsole с TableEditor вместо SQL editor/results;
connection/schema/relation определяют дедупликацию вкладки. TableChanges хранит
baseline и EditedRow: original, values (None = DEFAULT новой строки), deleted.
Двумерный viewport содержит редактируемые ячейки и номера для multi-selection.
Локальные изменения не вызывают worker. Один генератор statements обслуживает
preview и Command::Apply. Значения экранируются по диалекту, текст MySQL/SQLite
передаётся hex-литералами, PostgreSQL использует E-strings. WHERE включает исходные
значения для optimistic concurrency, включая старый PK и NULL predicates.
Apply использует SQLx transaction на одном соединении и требует ровно одну
затронутую строку на statement; ошибка/отмена до commit откатывает весь пакет.
MySQL требует InnoDB; серверное соединение после Apply выводится из пула,
SQLite progress handler прерывает VM по токену. После начала commit worker ждёт
его результата. Applied коррелируется по request ID, затем запускается reload
именно исходной вкладки. Ошибка оставляет локальные правки. Close gate учитывает
pending table edits; Save All не коммитит данные. DDL открывается отдельной
SQL-вкладкой с подсветкой и обычным сохранением файла.


`database_panel` маршрутизирует профили/тесты/SSH, сохраняет форму при ошибках и
сверяет тест с текущими полями. `sql_console` использует штатные TextBuffer и
TextEditorView, локальный SQL lexer и отдельные result tabs. `data_grid` хранит
порядок индексов строк, сортирует загруженный snapshot и рисует только пересечение
двумерного viewport. Копирование выполняется по исходному значению, preview ячейки
ограничен 256 символами. Результат и текст остаются при скрытии/смене вкладки.


## Системная интеграция RC1 (Phase 9)

`menu_bar::MENUS` — единый каталог идентификаторов, меток и availability predicates.
Он регистрируется в ActionRegistry и используется embedded egui MenuBar и
нативным macOS muda Menu. Callback AppKit кладёт ID в bounded очередь и будит
окно; dispatch на UI повторно проверяет текущий ActionContext. KeymapRegistry
поставляет подписи и native accelerators; нативные item properties обновляются
только при смене availability/keymap, а не на каждом кадре.

`integration` связывает actions с buffer/project/VCS/database/terminal API.
OpenPath и Create выполняются в fs_model worker: тип пути определяется вне UI,
create_new защищает существующий файл. CreateProject валидирует один компонент
имени, использует create_dir (без перезаписи) и открывает полученную папку.
ProjectOpened/ProjectCreated отделены от Tree (раскрытие подкаталогов), поэтому
история меняется только после успешного открытия; ProjectCreationError сохраняет
форму New для повтора. New выбирает parent через ОС, затем спрашивает имя.
`project_dialog` вызывает safe objc2 AppKit NSOpenPanel на главном потоке macOS
(файл/папка вместе); Windows/Linux используют rfd native/xdg-portal с отдельными
режимами папок и файлов. Отмена не запускает I/O и не меняет проект.

`recent_projects` хранит canonical PathBuf + Unix milliseconds в соседнем
`recent_projects.json`. Отдельный последовательный worker загружает, дедуплицирует
и атомарно записывает JSON через tempfile; повреждённые файлы не перезаписывает.
Reopen перемещает проект наверх, включая случай перевода системных часов назад.
История и элементы открытия в Project видны только без открытого проекта;
в hover submenu File → Recent Projects история доступна всегда,
включая native macOS menu. При shutdown очередь записи дожидается ack до 1s.

Cut/Copy обрабатывают штатные egui clipboard
события, RequestPaste читает clipboard в winit; menu paste восстанавливает фокус
последнего активного редактора. Recent Files ограничен 30 путями.

Project, Git, SSH, Database и Notifications используют единый левый dock и
типизированный `LeftTool`; Structure и правые rail/dock удалены. Menu/actions
Database, New Connection и SQL Console также открывают левый Database dock.

`layout::Layout` хранит относительные left/bottom/sql размеры; legacy right
сохранён только для совместимости существующего layout.json. Ordered worker
загружает и атомарно записывает соседний `layout.json`; UI сохраняет изменения
с debounce 250 ms, на shutdown ждёт final write до 1s. Только drag меняет dock
пропорции; resize окна пересчитывает размеры из пропорций. Ошибка чтения/JSON
не превращается в перезапись defaults. Native splitters egui и SQL drag bar
используют общие theme tokens.

CloseTarget (window/file/sql) → проверка dirty → Save All / Discard / Cancel.
Waiting state ждёт завершения всех отправленных save revisions; поздний ack
не очищает новые edits. При ошибке нет автоматического Close, при Cancel окно
остаётся. Непривязанные к файлу SQL-консоли получают Save path dialog. Clean close
и Discard освобождают Rope/history/result snapshots, Drop SQL отменяет query token.
Смена проекта сохраняет открытые документы. Повторное открытие пути получает
новый BufferId; идентификаторы не переиспользуются.

`app::main` создаёт event runtime, attach_runtime регистрирует bus/watch wakeup
и native menu на главном потоке. eframe intercepts close_requested/CancelClose;
NozhikUi::shutdown_systems идемпотентен (также вызывается из Drop): layout flush,
LSP shutdown→exit (≤3s), terminal manager (≤3s), database shutdown acknowledgement
(≤4s), SSH disconnect acknowledgement (≤3s), plugin host stop, abort wake bridge.
Database worker отменяет/завершает query tasks и ждёт Pool::close до 2s суммарно,
затем останавливает owned tunnels. SSH worker явно disconnects authenticated
handles с общим deadline 2s. После возврата run_native app дожидается consumer
до 2s и останавливает runtime. Жёсткий kill не проходит этот flow.

Idle: нет unconditional 100ms repaint. Ответы I/O, terminal grid, language service,
DB/SSH и bus/watch bridge вызывают request_repaint; debounce и Git timer —
request_repaint_after только при наличии срока. Для скрытого non-Git проекта
просроченный next_poll не планируется (регрессия zero-delay busy loop).
Plugin watch не публикует одинаковый snapshot каждую секунду. Background service
health timers не требуют UI frame. LSP и terminal курсоры пока без idle blink.
Стендовые стресс-замеры и ручные release gates — verification-checklist.md.


### Представление дерева проекта

ProjectTree::load читает только непосредственных детей на fs worker; сохраняет
исключённые пути в snapshot (is_excluded), используя rooted incremental ignore matcher
с родительскими/вложенными правилами. `.git` скрыта, symlinks не раскрываются.
FileIndex по-прежнему использует фильтрованный walker: ignored/service paths
не попадают в поиск. Watcher наблюдает отображаемые ignored paths, исключая `.git`.
UI project_icons рисует векторные папки, документы и chevrons без зависимости
от emoji-шрифтов; классифицирует тесты/линтеры по относительному пути. Красное
исключение имеет приоритет над зелёным test/tool состоянием, выбранная строка
сохраняет контрастный selection фон. Файлового I/O при рисовании нет.

## Оформление оболочки и локальные сборки (2026-10-26)

`theme::dock_frame` задаёт общий радиус 8 logical px, внутренний отступ 8 px
и внешние промежутки 2 px для Project/Git/Remote, Database, редактора и нижнего
окна. Фон оболочки CHROME виден в промежутках; rails имеют ширину 36 px и
векторные кнопки 28 px. `tool_icons` рисует смысловые контурные иконки через
egui Painter, без Unicode-глифов, внешних шрифтов или новых зависимостей.
Палитра различает тёмные tool windows, редактор и внешнюю оболочку.

`binaries/` целиком исключён из Git. `scripts/update-binaries.py` сохраняет
локальные release builds, bundle и metadata. После доработок по умолчанию
обновляется только текущая платформа; `--check` также проверяет только её.
Windows cross-build на Mac выполняется лишь по явному запросу пользователя через
`--target x86_64-pc-windows-gnu`; `--check --platform windows-x64` проверяет этот
комплект, `--check --all-platforms` — оба. Windows-кэш можно удалить через
`rm -rf target/x86_64-pc-windows-gnu`, сохранив binaries/windows-x64.
CI binaries.yml публикует только artifacts с ограниченным сроком хранения,
без записи в репозиторий. Новых функций интерфейса не добавлено.

## Code Styles / formatting (2026-10-27)

`config::code_style` владеет строгой JSON-схемой CodeStyle, встроенным PHP
профилем и CodeStyles catalog. Config.file_types хранит nullable имя JSON-профиля
и идентификатор syntax palette на расширение; отсутствие PHP/PHTML записи
выбирает встроенный профиль, явный None отключает форматирование и Smart Paste.
`language_support` теперь зависит также от config (типизированный CodeStyle).

SettingsDialog worker читает каталоги `~/.nozhik/code_styles` и соседний с
settings `code_styles`, сохраняет Config через ConfigManager на отдельном потоке.
UI получает готовый каталог целиком; reload failure сохраняет прежний каталог,
файлового I/O при отрисовке нет. Очередь сохранений последовательная, shutdown
ждёт ответы до 1s. General/File Types UI вынесен в settings_dialog.rs.

Command::Format несёт исходный snapshot, revision, token, профиль и optional
UTF-8 range. LSP actor сначала синхронизирует snapshot и проверяет capabilities,
затем запрашивает formatting/rangeFormatting с tabSize/insertSpaces. При отсутствии
поддержки или отказе запроса выполняется formatting::php_edits. JSON профиль
управляет локальным formatter; остальные vendor rules LSP задаются самим сервером.
Локальный formatter меняет промежутки между AST-токенами, сохраняет literals и
comments, повторно проверяет дерево и отказывает для malformed PHP. Heredoc/nowdoc
и non-PHP markup являются защищёнными узлами; их содержимое не переписывается.
Selection ограничивает все изменяемые диапазоны исходными границами.

UI сопоставляет ответ с BufferId/revision/выбранным профилем, затем применяет
одну EditTransaction. Smart Paste создаёт одну транзакцию для всех курсоров,
нормализуя каждый фрагмент относительно строки своего диапазона. Без профиля
вставляется исходный текст. syntax diagnostics передаются вместе с Highlights,
хранятся отдельно от server diagnostics и используют общий renderer волн/tooltip,
включая нулевой диапазон предполагаемого закрытия. Оба набора сбрасываются при edit.

Settings modal ограничивает content size размерами viewport с внешним запасом.
Правая панель использует явно вертикальную раскладку и bounded ScrollArea;
высота разделителя задана отдельно, без зависимости от unbounded Area.
На узких экранах категории переносятся над содержимым. Заголовок и Close
находятся вне прокручиваемой области и остаются доступны.

### Editor mouse input, formatting fallback and Smart Paste (2026-10-27)

TextEditorView captures LMB through one click_and_drag Response; visible rows use
hover only. ScrollSource enables wheel/scrollbars and disables content dragging.
Pointer coordinates use each visible row's origin and Galley; UTF-8 conversion
uses char boundaries. The temporary drag anchor is bound to BufferId/revision;
CursorSet remains authoritative. Double/triple clicks select identifier/line,
Shift extends the existing anchor. Selection changes request repaint.

reformat_code synchronizes language state before submitting a request, including
standalone files and the first invocation. Empty/no-op LSP edits invoke the local
AST fallback; nonempty LSP edits retain priority. PHP_ONLY handles tagless code
fragments. Whitespace edits crossing a selection boundary are clipped only when
preserving the unselected prefix/suffix, allowing a selected method call to regain
its indentation. The result still commits as one transaction.

formatting::paste_edit replaces the entire leading whitespace when the caret is
inside that prefix; single-line insertion therefore gets the target indentation
exactly once. Empty target lines use the preceding nonempty line as context.
A bounded PHP clipboard lexer computes structural nesting, excluding quotes,
escapes and comments. This is not document parsing on the UI thread. Multiline
literals/heredocs and empty clipboard text bypass normalization. Without a style,
the original range and clipboard text are unchanged.

### Enter indentation and partial LSP formatting (2026-10-27)

TextEditorView routes Enter (after completion acceptance) through one transaction
with per-cursor newline_text. Only the target Rope line/prefix is materialized;
no document parser or full-buffer String is used for Enter. The clipboard lexer
also supplies remaining bracket depth, ignoring quoted/commented delimiters.
The configured indentation width/style and the line's LF/CRLF are preserved.
Without a profile, the transaction contains only the newline.

Nonempty server formatting edits are validated for overlap and applied to a worker
copy of the source. normalize_lsp_indentation then uses php_edits to repair only
existing line-leading whitespace; server token/spacing changes retain priority.
Selected-range boundaries are mapped through server edits and remain enforced.
The combined result becomes one original-coordinate UTF-8-safe replacement,
therefore the existing revision/profile gate and one-step Undo are preserved.
Unsupported local syntax keeps the server result. Empty/no-op LSP responses still
use the complete local formatter.

### Heredoc/nowdoc and mixed PHP documents (2026-10-27)

The local formatter no longer rejects a whole file containing heredoc, nowdoc or
markup. Heredoc/nowdoc/string nodes are opaque; text_interpolation preserves the
complete PHP-close/markup/PHP-open interval. Gaps beside markup/tag transitions
and mixed-document outer whitespace are preserved. PHP token gaps elsewhere still
use CodeStyle and AST scope depth, including blocks spanning multiple PHP islands.
Reparse validation compares both the AST shape and ordered raw protected payloads.

Before accepting server formatting, normalize_lsp_indentation checks that literal
and markup bytes are unchanged. A server result that rewrites a payload falls back
to local formatting of the original snapshot. The caller still applies one revision-
guarded transaction. The supplied php82-smoke.php is a data-only regression fixture;
its PHP code is parsed/formatted, never executed by the Rust test suite.

## Offline PHP catalog and completion (2026-10-27)

`language_support::php_stubs` owns schema v1: `PhpVersionStubs` contains typed
`PhpSymbolKind` entities and `PhpParameter` declarations. Classes/interfaces own
public methods and constants; function/method snippets support one `${0}` caret
marker. serde is now an explicit language_support dependency. The embedded
`data/php_<version>.json` contains the generated runtime inventory for each of
11 supported levels, 7.0–7.4 / 8.0–8.5. All are embedded with include_str; 8.5
remains the default. catalog-manifest.json records exact patch versions, image
IDs/digests, extensions, entity/member counts and source/output SHA-256 hashes.
Coverage is the official CLI Alpine image's loaded extensions, not every optional
PHP extension. Values of platform-specific constants describe Linux/amd64.

Config.php_language_level defaults to 8.5 and accepts 7.0–7.4 / 8.0–8.5. The
Settings worker loads `~/.nozhik/languages/php/versions/php_<level>.json` when PHP
is first needed (an open PHP document or Settings), and on level changes. Full
catalog parsing runs only on the worker, including the initial bundled inventory;
an empty workspace does not load PHP catalogs or schedule polling for them. Only NotFound uses the bundled catalog of the same requested
level; malformed/mismatched user catalogs report an error and retain the previous
catalog. Replies carry the requested
level so stale loads cannot replace the latest selection. No filesystem I/O runs
in the popup render pass. The UI shares immutable catalogs via Arc.

Identifier input searches the current line prefix immediately; Ctrl+Space also
allows an empty prefix. Global functions/types/constants/keywords and explicit
`Class::member` names are supported. Instance type inference and user-code symbol
indexing remain LSP responsibilities. LSP responses add project suggestions and
are merged with offline matches by label; revision/caret/token gates still apply.
Esc, outside clicks and completion acceptance cancel pending requests. Offline
insertion replaces the UTF-8 prefix in one buffer transaction, preserves a leading
namespace separator and positions the caret at `${0}`. LSP snippets remain disabled.

The Foreground egui Area uses a 500 px surface (clamped to narrow viewports),
9 visible 28 px rows, vertical scrolling, colored round icons, accented prefixes,
clipped signatures and right-aligned types. Hover and single click select; double
click/Enter/Tab apply. Keyboard selection scrolls the active row into view.

`scripts/generate_php_stubs.php` supports PHP 7.0+ Reflection, including parameter
flags/defaults, tentative return types when available, public class/interface
members and all loaded extension constants. The generating runtime/image and its
extensions determine coverage. It never invokes discovered functions/methods.
Output uses a temporary sibling file and rename; the Rust `validate_php_stubs`
example validates a generated catalog before installation. Docker is only a
manual generation path; the IDE neither installs nor launches containers.

`scripts/generate-php-catalogs.py` regenerates all levels sequentially, using one
Docker container at a time. Successful containers use --rm; failure cleanup is
limited to the uniquely named container created by that invocation. Newly pulled
image tags/unused layers are removed after each version by default; pre-existing
images and unrelated containers/volumes/caches are preserved. No global prune is
used. Per-version provenance is saved only after schema/coverage/version checks.

Offline completion ranks case-matching names before other case-insensitive
matches, so lowercase `str` includes strlen/str_contains before the many STREAM_*
constants exhaust the 100-item limit. Uppercase constant queries retain priority
for uppercase names. LSP sortText semantics are unchanged.

## AI Inline Completion / Ollama (2026-10-27)

`nozhik-ai-completion` owns bounded Unicode FIM prompts, a reusable reqwest
HTTP transport (Rustls, JSON, streaming), tags/pull APIs and an owned cancellable
Generation. Dependencies are tokio, reqwest, serde/serde_json, thiserror and Unix-only nix;
no UI/config/filesystem dependencies. Typed errors follow repository standards.
`ui` depends on ai_completion; config owns persisted ai_enabled/ai_model/
ai_debounce_ms fields. app attaches its existing runtime to the UI adapters.

The editor gates replies by BufferId, revision, caret and focus, cancels the
owned debounce/HTTP task on input/navigation/pointer/focus/settings changes,
and schedules once after opening/refocusing a document or a changed caret/revision, including idle stops. Rope extraction is bounded to 3000
scalars. A dropped Generation aborts both sleep and in-flight HTTP; results use
oneshot + event-loop wake, without continuous polling or error logging. GenerationError categories are shown
in the status bar; transient failures get two delayed retries and then settle. No automatic download. Enabled AI starts an application-scoped service lifecycle (see below).

Ghost text lives outside TextBuffer. Virtual rows and LayoutJob sections insert
muted translucent text at the caret and move the existing suffix/following rows;
syntax colors are retained on real text. Tab commits one ordinary transaction
and clears the PHP/LSP popup; other input dismisses. Multiple cursors/selection
suppress AI. The main editor supports all text file types; SQL tool consoles do
not enable AI in this iteration.

Settings model management uses runtime tasks and a watch channel that coalesces
NDJSON progress instead of accumulating an unbounded queue. Model changes abort
old inspections. Pull locks the model name, reports current-layer byte counts,
requires terminal success and then refreshes tags. Service refresh is explicit;
no idle polling. Generation uses raw Qwen FIM with the required stop tokens,
stream=false, a 60 s cold-load deadline, temperature=0.2 and num_predict=64. Pull has a separate long
deadline; chunk boundaries need not coincide with JSON/newline boundaries.


### Ollama application lifecycle (2026-10-27)

app owns ai_completion::lifecycle::Lifecycle, starts it from persisted AI settings
before the native window opens, and awaits shutdown after eframe returns, before
stopping Tokio. ConfigChanged (including Lagged resync) coalesces model/enable
updates via watch. Startup, model loading and shutdown use background async I/O;
normal editor frames do not poll lifecycle state. UI shutdown first cancels AI
generation/download tasks.

Enabled startup inspects tags, launches `ollama serve` only if unavailable,
searches PATH and standard macOS/Windows installation paths, and preloads only an
installed selected model using an empty-prompt generate request. No Ollama install
or model download is performed automatically. Switching models/disabling AI and
normal application close unload the selected model with keep_alive=0. Preload
and generation retain the standard five-minute idle expiry as a crash fallback.
An external Ollama server is never killed; the selected model is still unloaded.
Owned Unix server/runners share a fresh process group and are killed/reaped as a
group; Windows uses taskkill /PID /T /F only for the owned live child, then reaps
its process handle. No process-name kill or global Ollama shutdown is used.
Startup/preload are cancellable, model unload has a five-second deadline, and
owned server termination follows even if HTTP cleanup fails. User-installed
models stay on disk. Abrupt OS kill/power loss cannot guarantee graceful unload;
external service residency remains bounded by the normal five-minute idle timeout.


### Idle-caret AI and native pointer input (2026-10-27)

Inline state tracks focus/identity, an attempted flag and bounded retries. New
files/tabs request editor focus, so initial idle scheduling does not require a
keystroke. An unchanged completed request does not run again; Esc suppresses the
current position. A later caret/focus/revision/settings change rearms generation.
Generation returns typed errors instead of silently discarding failures; a missing
model directs the user to Settings/Download Model. No automatic download was added.

The native smoke exposed press+release delivered inside one egui frame. In that
case pointer.press_origin is already cleared and is_pointer_button_down_on is
false. Editor hit-testing now also accepts Response::clicked_by and falls back
to interact_pointer_pos, preserving drag/selection handling and allowing a fast
click to place the caret (and trigger idle AI) in an empty indented line.

## Контекстная документация (2026-10-27)

`language_support::doc_provider` задаёт языконезависимые `EntityKind`,
`SymbolDocInfo`, `LanguageDocProvider` и `DefinitionTarget::{LocalFile, ExternalUrl}`.
`PhpSymbolKind` остаётся совместимым alias общего EntityKind. LocalFile резервирует
native path и zero-based line/UTF-8 column для будущего project index; текущий
PHP provider выдаёт ExternalUrl. LSP navigation сохраняет существующий контракт.

`PhpVersionStubs` реализует exact lookup активной версии: функции/типы/методы
сравниваются без ASCII-регистра, константы — с регистром; поддерживаются FQN и
Class::member. Правила DocRules читаются worker вместе с каталогом: встроенный
php_doc_rules.json либо пользовательский sibling doc_rules.json рядом с versions/.
Правила отделены от portable stubs schema, не записываются в JSON каталога.
HTTPS templates, case/underscore transforms и fallback выбираются по категории;
подстановка percent-encodes небезопасные байты имени. Ошибка правил не подменяет
пользовательские данные defaults и сохраняет старый snapshot в SettingsDialog.

LanguageUi связывает PHP-каталог с DocumentationSource (language_id, label,
Arc<dyn LanguageDocProvider + Send + Sync>). EditorView использует только этот
контракт для инспекции; php.net и правила URL в renderer отсутствуют. Hit testing
опирается на видимые Galley, UTF-8 границы и фактический glyph под указателем;
отступы, gutter, punctuation, переменные и известные string/comment spans не
инспектируются. Модификатор выбирается по target OS, освобождение скрывает popup.
Area имеет Tooltip order, Darcula frame, ограниченный viewport и встроенный
JetBrains Mono. При переходе press/release не меняют CursorSet; view передаёт
типизированный target оболочке. Системный webbrowser::open выполняется на отдельном
именованном потоке; ошибки пишутся в stderr, UI не ждёт процесс браузера.

Новая прямая UI-зависимость webbrowser использует уже существующую транзитивную
версию eframe. JetBrains Mono Regular и SIL OFL находятся в crates/ui/assets/fonts;
именованное семейство используется только для документации, шрифт редактора
не меняется. Разрешение project shadows/import aliases и типов экземпляров
остаётся задачей LSP/будущего индекса, lookup имени этого не заменяет.

### Smart deletion of whitespace-only editor lines (2026-10-27)

TextEditorView routes unmodified Backspace/Delete through replace_inner with the
active CodeStyle and fallback tab width. blank_line_deletion reads the target
Rope line and walks back to the preceding nonblank line; it uses the same
newline_text indentation rule as Enter, or previous indentation without a profile.
No full-buffer materialization, parser or I/O is added to the input path.

If the caret's logical column exceeds the expected indentation, the edit deletes
only excess whitespace before it. A tab crossing the target column is replaced
by the necessary spaces; the suffix remains untouched. At/before the expected
column, the complete blank line and its preceding LF/CRLF are removed, putting
the caret at the preceding line end. The first line consumes its following
newline instead. An empty final buffer is a no-op. Selections/nonblank lines and
modified keys retain ordinary deletion. All cursor edits use original-byte
coordinates, overlapping blank-line deletions are merged and one transaction
preserves the normal Undo/Redo and revision contracts.

## Differential project symbols (2026-10-27)

language_support::{symbol_index, project_indexer, triggers, type_inference} adds
language-isolated SymbolIndex instances (PHP/Go/JavaScript identity; PHP extraction
only). php_symbols owns Tree-sitter AST extraction off the UI thread. Per-file
facts retain declarations, lexical scopes, namespace/import contexts and reference
candidates with UTF-8 coordinates and line previews. ASTs are not shared with UI.
DeclarationsByFile and ReferencesBySymbol are authoritative linked tables.
Candidate→source-file dependencies let declaration additions/removals relink
already-parsed dependent facts. Replacing/removing a file removes its old outgoing
edges and updates counts only for touched identities; no dependent file is parsed.

ProjectIndexer is a single-owner dedicated thread with commands Changed(path),
Overlay(path,revision,Rope), Close, Reconcile and Invalidate. Wake callbacks and
bounded event drains deliver Arc snapshots/progress/errors; no UI filesystem I/O.
Disk facts/stamps and unsaved overlay facts are separate: cache writes never include
unsaved text. Startup loads versioned, root-bound index_php.json, then reconciles
nanosecond mtime + length. An atomic same-directory tempfile replaces the cache.
ignore and tempfile are now explicit language_support dependencies. Traversal
includes ignored supported sources and skips only .git/.nozhik internals (policy clarified 2026-10-29).
Watcher paths and successful save replies enqueue Changed; Lagged reconciles
metadata without reparsing unchanged files. Index lifetime follows project root;
buffer overlays outside that root are rejected. Drop cancels traversal/work.

LanguageUi gates editor snapshots by overlay revision. Local completions precede
catalog results; member contexts suppress unrelated global/LSP items. Editor input
reads LanguageTriggerConfig rather than embedding operators. LocalFile navigation
preserves UTF-8 columns and converts to UTF-16 only against the opened source.
Virtual code-lens rows above class/method declarations share viewport mapping with
AI ghost rows. Alt+F7 and code lenses open usages_popup with keyboard/mouse routing,
file/code filtering, file grouping, accented match ranges and an immediate egui
viewport for the separate window. Modal state blocks underlying editor mutations.
Unused name ranges split syntax jobs and use #7A7E85; tooltip states the inspection.

Current PHP resolver covers direct new assignments, typed parameters, $this, namespaces,
class aliases/grouped imports, exact globals/static/instance calls, properties and
class constants, parent classes/interfaces and parent:: dispatch. It does not resolve
trait adaptations, arbitrary chained return objects, union/intersection flow types
or dynamic calls. Unused inspection is
based on indexed references, including public members; it is not proof that a
framework will never call a symbol. Go/JS/TS parsers and caches await their workers.

### PHP class references, hierarchy and reverse navigation correction

PHP extraction explicitly records class-name locations for imports, extends/
implements, type hints, new/instanceof, attributes/catch and static scopes. Counts
represent source type references, not the sum of a class's member calls. A class
reference and a method reference in Foo::run() have distinct source ranges.
Grouped use aliases resolve against the group prefix; comments/literals are excluded.
Class declarations retain direct base_types. SymbolIndex::resolve walks a cycle-safe
class/interface lineage and returns the actual member declaration; private inherited
members are excluded. Completion filters overridden/inaccessible inherited members.
Simple $this->typedProperty receivers are resolved by the PHP extractor too.

resolved_by_file retains actual outgoing targets (which may be in a parent), and
dependencies_by_file retains raw member/class/ancestor dependencies, including
missing parents. Replacement removes actual old targets, then relinks only dependent
saved facts. Removing an override, changing a parent or declaring a missing class
never reparses callers. Counts and popup rows use the same reverse buckets.
Cache schema 2 discards schema-1 extraction/stamps and rebuilds once on startup;
subsequent edits/restarts retain differential behavior. Build-input hashing includes
the example PHP fixture consumed by Rust tests.

Cmd/Ctrl hover on a declaration queues usages, not a self-definition/documentation
popup. The persistent window remains clickable after modifier release and wakes
navigation on selection. A per-view hover latch prevents reopening immediately after
Esc/close; tab activation rearms it. Reference clicks still emit LocalFile targets.
The end-to-end egui regression uses actual pointer press/release, background file
opening, caret bytes and painted/visible target rows to cycle through inherited
methods, classes/aliases and properties in both directions, without editing buffers.

## Language policy and selective/batched indexing (2026-10-29)

config::languages owns LanguageId, language/extension registrations and availability.
Only PHP/PHTML is implemented. JS/TS, HTML, YAML and Go have unavailable registrations;
they cannot accidentally activate a parser. SymbolIndex re-exports the shared ID;
serde aliases preserve existing Php/JavaScript/Go cache values. Config.enabled_languages
is a typed set, defaults to compiled available languages, accepts an explicit empty
set, and persists through the existing TOML/JSON settings worker. Unknown IDs remain
syntax errors. Settings → Languages has immediate checkboxes and unavailable markers.
New backends register their identity/extensions here when their service is wired in.

LanguageUi compares root and language policy, drops old workers/views/tokens/usages
on policy changes and starts only selected available backends. Disabling PHP prevents
index traversal/cache reads, LSP and catalog requests, removes semantic editor state
and language formatting/style behavior. Pending old catalog replies are ignored.
The cache is retained. Re-enable reloads cached disk facts and overlays open buffers;
unchanged disk snapshots are not serialized again. All-off is plain text; AI and
filename navigation are separate features. Filesystem FileIndex still inventories
all non-internal names for Go to File, including ignored files, not source contents.

ProjectIndexer applies the same registry filter to discovery, Changed and Overlay,
before reading source text. Directory entries must still be enumerated for discovery;
only enabled supported source paths contribute to progress totals, stamps and facts.
Only .git/.nozhik internals are excluded; ignore rules affect tree coloring, not semantic discovery.
Disabled worker entry points do not read even an invalid cache. Commands report actual
changes so unsupported files, unchanged mtimes and unrelated cache notifications do
not serialize the project or clone/send another full snapshot. Reconciliation batches
changed parsed facts instead of linking after every source; partial-file updates remain
differential. Disk serialization borrows index/stamps rather than cloning them.

SymbolIndex::replace_files installs all batch declarations before linking candidate
references and hierarchy dependencies. A derived FQN→(path,position) hash lookup
replaces repeated whole-declaration scans; deserialization reconstructs it automatically
without parsing and does not persist it. Existing cache schema 2 remains compatible.
Cache restoration and no-op startup leave the cache's mtime unchanged.


### Ignored sources and PHP cross-file receiver linking (2026-10-29)

Schema 3 rebuilds older extraction facts once. All supported enabled source files
inside the project participate in discovery, including vendor/node_modules/target
and ignored paths. Ignore policy controls display only. Traversal disables .ignore,
.gitignore, parent/global/exclude filters, never follows symlinks, skips only
.git/.nozhik internals, and reports traversal errors instead of silently dropping
files. No Composer configuration or Git membership is required to index a source.

FileIndex likewise includes ignored names without parsing their contents. Tree
coloring uses vcs::project_paths::ignored_paths: an isolated read-only Repository/
Index belongs to the calling filesystem worker, and only owned path sets are returned.
Tracked files and tracked directory prefixes override ignores, using index binary
prefix lookup rather than scanning all tracked files per displayed row. Native Git
ignore evaluation decides remaining repository paths; non-repositories retain
ignore-matcher fallback. fs_model now depends on vcs for this read-only classification;
vcs has no fs_model/UI dependency. Tree load/search/index never mutate the Git index.

Promoted constructor parameters produce both a member and a local parameter.
PHP extraction retains bounded AST-free PhpTypeSource expressions for assignments
and member receivers. Linking resolves typed inherited properties, declared return
types, call results, static factories and parenthesized object construction using
project declarations. Missing receiver/member/ancestor dependencies are retained
for differential relinking; a dependency edit never reparses its callers. at()
uses actual resolved outgoing edges, matching reverse usages exactly. PHPDoc typed
tags contribute exact UTF-8 references; ordinary comments/descriptive text do not.

Invalidate seeds the live index with retained overlays before emitting their revision
gates. Reconciliation installs disk facts without temporarily advertising valid
buffer revisions with missing facts. Unsaved contents remain excluded from disk.


### Editor tab strip (2026-10-29)

`ui::editor_tabs` renders file and SQL tabs in one horizontal ScrollArea with
fixed trailing overflow/actions controls. Tab rectangles and the actual scroll
viewport determine overflow-menu membership on each frame, including partially
visible tabs on either edge. Selection reveals the whole tab; manual scrolling
is retained until the active buffer changes. The strip uses zero clip expansion
so reveal coordinates and overflow classification share the same boundaries.

Go to File opens the existing Search Everywhere Files filter. File-only bulk
closure uses CloseTarget::Files and the existing dirty/revision save gate;
Save All in this flow saves only open file documents, with SQL destinations left
outside the operation. Close Unmodified removes clean files in reverse order via
the same close lifecycle, retaining modified buffers and active indices.


Overflow popup dimensions follow measured file labels and row count, capped at
half the central editor region in each axis (including frame margins). Explicit
content bounds override egui Area's remembered size, allowing both growth and
shrinkage while open. Rows remain single-line with ellipsis and full-path tooltips;
a vertical ScrollArea handles lists taller than the limit.

### Terminal tabs, selection and dock height (2026-10-29)

Terminal tabs share the editor-tab painter (font, padding, active/hover colors,
rounded top corners and close hit area). The single scrolling row has an inline
new-session button and fixed trailing vertical actions/Hide controls. Close All
removes sessions through TerminalManager; Hide retains them. Opening an empty
panel creates one local session, including without a project (application cwd).
Empty panels do not respawn shells every frame after Close All or process exit.

TerminalView owns per-session cell-boundary selections, paints them with the
shared selection color and extracts Unicode text without wide-cell duplication.
Drag/Shift-click and a context menu provide Copy/Cut/Paste. Native Edit actions
route to the focused terminal before editor actions. Paste uses egui-winit's
clipboard request and existing bracketed-paste encoding; background/disabled
terminals never consume clipboard input. With a selection, Copy never sends
SIGINT. Without selection, Ctrl+C retains shell interruption. Cut edits a selected
range on the live cursor row through line-editor cursor/backspace bytes; history,
multiline output and alternate-screen selections are copy-only. The PTY remains
a stream owned by the shell rather than an editable output document.

Initial bottom height is 40% of the window, leaving a larger editor region.
Existing stored proportions remain respected. User resizing may use the available
window height; seed and persistence retain the release-frame geometry so hiding
and reopening restores the selected height. Layout validation accepts the enlarged
bottom range while retaining the previous side-dock limits.


### PHP member completion and stable edit presentation (2026-10-29)

Class facts now retain direct trait_types separately from base_types. Lookup visits
class declarations, recursively composed traits, then ancestors, with cycle guards;
private trait members are accessible only from their consuming class. Completion
uses that same resolution order to suppress overridden duplicates. Cache schema 4
rebuilds older facts once. Trait adaptations remain outside this bounded resolver.

EditorLanguage keeps Rope snapshots and a presentation copy of per-file symbols.
Before painting an edited revision, UTF-8-safe Rope differences move highlight ranges,
unused-name ranges and lens lines together. Worker results replace the presentation
without first erasing it. Diagnostics and navigation still require current revisions;
presentation facts never authorize a stale jump. Local member completion may use the
last index with caret coordinates mapped back to that snapshot until the new overlay
arrives. No parser, file I/O or full-buffer String is added to the paint/input path.
The completion Area explicitly resets both inner bounds, sizes to up to nine 24-point
rows and measured labels (440-point width/45% screen height caps), and scrolls longer lists.


### Git diff syntax and expandable context (2026-10-29)

FileDiff retains the bounded three-context-line patch and optionally full_hunks
from the same target (HEAD/index/worktree or parent/commit), prepared on the VCS
worker. A complete-context comparison exceeding the existing 2 MiB/50,000-row/
4096-character-line limits is discarded, preserving the original display patch.
DiffView uses the complete hunks when available, initially showing changes plus
three adjacent rows and explicit gaps. Each gap offers the applicable directions
and reveals up to 20 rows per click; adjoining regions merge without duplication.
Rebuilding visible rows clears selection to avoid stale row coordinates. Identical
normalized snapshots are ignored, retaining expansion/colors/selection during status
polling.

DiffView's syntax helper parses immutable old/new PHP snapshots on a background
thread using the existing language_support Highlighter, without starting LSP or
opening editor buffers. Deletions use old captures; context/additions use new
captures, clipped to UTF-8 line ranges. Partial hunks stay plain. Replacing the
file drops its receiver so stale replies cannot install colors. Config's shared
language registry, enabled_languages and file_types syntax palette govern parsing
and painting. egui lays out only visible colored rows; selection/copy and both
scroll axes remain available. No new crate dependency or source file mutation.


### Git History actions and synchronization (2026-10-29)

`vcs::history_actions` handles owned `HistoryRequest` commands on the serial worker.
Each dialog captures expected HEAD and branch; both are rechecked before execution.
Single actions implement staged-only amend (retaining author/unstaged contents),
soft/mixed/hard reset, safe detached checkout, first-parent revert, branch/tag creation
without switching, tag copying and validated github.com origin commit URLs.
History snapshots walk first parents and decorate commits with their parent count/tags;
non-commit tags do not break history. UI keeps a stable SHA selection anchor, supports
Shift ranges/Cmd-Ctrl toggles, and right-click preserves an existing group.
`SelectionDetails` returns one owned `HistorySelection`: selected commit metadata
and a sorted union of `HistoryFile` entries. Each entry retains newest-first
contributing commit IDs, newest delta, current worktree availability and an optional
encoded historical GitHub URL. The UI deduplicates paths and shows each file's
newest selected commit diff, preserving the existing full-context/syntax path.
Badges/tooltips describe other selected contributors, with no composition across
skipped commits. Selection changes, empty selections and pruned history invalidate
outstanding details/diffs using the existing request and generation gates.
`vcs_panel::history_files` supplies a read-only context menu: local open/reveal use
the current worktree, clipboard uses native absolute/relative paths, GitHub links
use immutable commit IDs (first parent for deleted files). Missing working copies
and unsupported origins disable the corresponding actions; historical inspection
never restores files or mutates refs/index.

Cherry-pick, contiguous squash and reorder preflight patches in temporary in-memory
indexes; conflicts leave refs, worktree and the disk index unchanged. Unattached Git
objects may be written during preflight. Reorder publishes a complete first-parent
suffix through HEAD; the UI extends that suffix when moving down. Merge patches use
mainline 1 and replay flattens merges; root replay/revert/squash are rejected. Rewrites
and replay/revert require clean saved state; existing editor buffers stay retained.
There is no force-push, hook/GPG or interactive conflict continuation for replay.
Dialogs retain worker errors and block global shortcuts through the existing modal gate.

The origin header chooses Fetch/Publish/Pull/Push/Pull & Push from upstream state,
with separate down/up count badges. Counts use the fetched tracking refs; local status
polling does not fetch automatically. `PullPush` requires a clean origin-tracking
branch, fetches, fast-forwards or merges divergence, then pushes without force. A merge
conflict opens the existing resolution dialog and leaves push for an explicit action
after Continue. Worker errors also keep merge state available to the existing banner.


### Project tree header controls (2026-10-29)

ui::project owns pending recursive expansion and file reveal alongside the existing
expanded/selected paths. Directory rows now select their path as well as toggle.
Select Opened File expands only the local active document's ancestors and scrolls
its painted row into view after lazy loading. SQL/SFTP/outside-project documents
are disabled targets. Expand Selected walks known child snapshots and schedules
LoadDir through the existing serial FS worker, with at most 16 outstanding UI loads.
It opens ancestors too, so a retained hidden selection becomes visible. No disk
traversal or canonicalization runs on the UI thread; symlinks remain excluded by
the existing tree loader. Collapse All clears expansion and pending reveal/recursive
intent; late snapshots populate the cache without reopening rows. Project changes
and worker errors clear pending intents. Hide uses the existing left-dock state,
retaining documents, selection and tree cache. Four vector header icons have
English tooltips and accessibility labels and remain outside the scroll viewport.


### Project launcher and repository cloning (v0.1.1, 2026-10-29)

Default startup no longer treats cwd as a project. NozhikUi renders a compact
launcher until the filesystem worker supplies ProjectOpened/ProjectCreated.
The existing ordered recent-project worker supplies the searchable MRU; failures
leave the launcher visible. Explicit CLI project arguments still open directly.
The same viewport grows to the IDE on successful open. Launcher navigation contains
only Projects/About and uses the existing embedded app icon and Cargo package version.

UI owns session-only Zeroizing credential forms and a single pending operation.
Dedicated threads run blocking vcs::clone or a current-thread Tokio runtime for
reqwest GitHub REST calls. GET /user validates the account; /user/repos pages of
100 include owner/collaborator/organization access (10,000 entry safety limit).
Requests have 20-second timeouts and refuse redirects. No OAuth client registration
is required: connection uses a user-provided PAT. No Enterprise endpoints or disk
credential storage. Late cancelled replies cannot open a project. Worker completion
requests repaint; no idle polling repaint is added.

VCS clone accepts public/helper/agent, token, basic credentials and explicit SSH key.
Secrets never enter persisted URLs or error text; callbacks refuse credential-bearing
redirects and bound authentication retries. libgit2 retains certificate/host-key checks.
Clone runs in a temporary sibling, checks cancellation, reserves the final directory
exclusively and moves the completed checkout there. Existing destinations are refused;
transport errors/cancellation clean temporary data. A filesystem failure during final
publication may leave a partial destination; its user-visible error is retained.
No submodule recursion or persistent credential binding for later fetch/push is added.

Dependency boundaries: ui adds reqwest for GitHub; vcs adds zeroize for transient
credentials and tempfile for clone staging. Neither adds shell Git dependence.
Version is inherited from workspace.package by all crates and app bundle metadata.


## Multi-project window (v0.1.3)

`Workspace` is the eframe application and owns a launcher plus independent `NozhikUi`
project sessions. Each session owns its EventBus, ConfigManager, filesystem watcher,
Git/language/remote/database workers, editor buffers, terminal tabs and panel layout.
Only the active session renders; inactive workers still drain bounded replies.
A single native menu moves with the active session. Session buffer-ID ranges avoid
cross-project egui editor identity collisions; tree scrolling is keyed by root.
The application-scoped Ollama lifecycle follows the active project's config.

Open requests from pickers, menus, MRU and clone results route through Workspace.
A worker canonicalizes the path and validates `<root>/.nozhik/settings.toml` before
constructing a session. Missing settings use Config::default, not global overrides;
malformed settings never overwrite the source. File paths open in the current editor.
A canonical root is opened once; duplicate requests activate its existing session.
Settings writes remain on SettingsDialog's ordered worker and use ConfigManager's
atomic persistence. The dialog labels the project name and root. Layout is project
local; one launcher-owned worker writes the shared recent-project history.

The project menu and horizontal tab strip queue switch/close/reorder intents.
Reorder preserves active identity. Project close uses the existing revision-aware
Save All / Discard / Cancel gate, then drops only that session and its resources.
Window close visits every project, including inactive dirty buffers; Cancel aborts
that sequence. Closing the final project normally returns the retained launcher.
macOS uses a transparent titlebar with native window controls and a draggable
header background; project-tab dragging is separate from window dragging.


### Project tab scrolling and live reorder (v0.1.4)

The project row has a fixed 32-point height and a bounded 28-point horizontal
viewport. Its ScrollArea is AlwaysHidden with scrollbar/drag scrolling disabled;
only wheel/trackpad input and two overflow-only vector arrow buttons change the
offset. Tab widths derive from the viewport rather than the scrolled canvas.
A drag stores the canonical project path and press origin. Crossing a neighbor's
midpoint queues a reorder while primary is held; paths retain gesture identity
across changing indices. The active project stays selected and each reorder
requests repaint. Close targets do not start drags and drag release does not
activate the moved inactive project.


## Нижние инструменты в левой полосе (v0.1.5)

`app_ui::panels` выделяет status bar, затем полную по высоте `tool_rail`,
затем нижний dock и боковые tool windows. Поэтому Terminal/Problems, дерево
и редактор всегда находятся справа от 36 px полосы. Отдельной горизонтальной
строки переключателей нет. Верхняя группа инструментов сохраняет прежний
порядок; bottom-up layout закрепляет внизу квадратные 28 px векторные кнопки
Terminal и Problems. Переключение сохраняет прежние bottom state, lazy PTY,
фокус, сворачивание и persisted resize; Problems пока остаётся placeholder.


## Git chrome справа от полосы инструментов (v0.1.6)

`app_ui::panels` создаёт Git toolbar и merge banner после полной левой полосы,
перед dock-панелями. Полоса больше не сдвигается Git-заголовками; все Git-окна
начинаются справа от неё. `VcsPanel::toolbar` самостоятельно задаёт горизонтальный
layout высотой 50 px: выбор ветки, Fetch и контекстные сетевые действия.
Current Repository с повторным именем/иконкой удалён; имя проекта уже во вкладке.
Fetch остаётся отдельной видимой кнопкой, если primary action — Pull, Push,
Pull & Push или Publish. Сетевые операции сохраняют прежние background tasks
и ограничения origin/operation/mutation; отрисовка не вызывает сеть.


## Единые иконки проектов и верхняя строка (v0.1.7)

`project_badge` рисует инициалы для welcome rows, project switcher и project menu.
Один детерминированный hash первых трёх Unicode-символов имени без учёта регистра
выбирает один из 12 ярких цветов; путь, порядок MRU и активность не влияют на цвет.
Project tabs используют строку 36 logical px с симметричными отступами по 4 px
вокруг прежнего 28 px viewport. Внутренняя строка вкладок выровнена по верху
(horizontal_top), чтобы вертикальное центрирование не выталкивало вкладки
за viewport; скрытая прокрутка и drag/reorder не меняются.
Current File удалён из top toolbar, остальные кнопки сохранены.


## Лицензирование и материалы в поставке (v0.1.10)

Собственный код Nozhik — Source-Available (корневой LICENSE); workspace packages
наследуют license-file, а не SPDX Apache-2.0. Подписка/активация не добавлены.
Покупка на одного пользователя, без ограничения устройств; самостоятельная
сборка для себя и внутренней работы организации разрешена. Сторонние компоненты
сохраняют лицензии. CONTRIBUTING.md ссылается на права для коммерческих сборок
и перелицензирования вкладов без передачи авторства.

update-binaries.py вызывает third_party_licenses.collect после Cargo build:
locked/offline metadata с filter-platform определяет пакетный граф. В поставку
и Resources/Legal bundle входят LICENSE, THIRD_PARTY_NOTICES.md, сгенерированные
THIRD_PARTY_LICENSES.txt и THIRD_PARTY_SOURCES.zip. Архив содержит оригинальные
исходники MPL-пакетов и пакетов без отдельных license files; тексты лицензий
собираются также из вложенных native libraries и fonts. Граф может включать
build-time/неиспользуемые пакеты. Сборка не меняет исходники зависимостей.
Legal inputs включены в source fingerprint, payload hashes покрывают sidecars
и подписанный app archive; CI реагирует на изменение legal inputs.
Новые зависимости/скопированные ресурсы и Windows GNU runtime требуют отдельной
проверки условий: collector не устанавливает совместимость лицензий.
