# Архитектура Nozhik — Phase 9 / v0.1.0-RC1

## Границы крейтов

| Крейт | Ответственность | Внутренние зависимости |
| --- | --- | --- |
| nozhik-core | Значимые типы и их инварианты | Нет |
| nozhik-event-bus | Системные события и транспорт pub/sub | core |
| nozhik-buffer | Rope, координаты, курсоры, история, поиск | core, event_bus |
| nozhik-config | Схема, валидация, хранение и доступ к настройкам | core, event_bus |
| nozhik-fs-model | lazy tree, ignore, file index, watcher, I/O worker | event_bus |
| nozhik-vcs | Git repository/index, status/diff/history, branches/remotes, serial worker | git2, thiserror |
| nozhik-plugin-host | Manifest validation, isolated process lifecycle, action/keymap registries | event_bus, tokio, serde/serde_json, semver, directories |
| nozhik-language-support | PHP incremental syntax, stdio LSP client, workers, UTF-16 conversion | tree-sitter/php, lsp-types, tokio, ropey, serde_json, thiserror |
| nozhik-terminal | Native PTY, VT100 grid/scrollback, background sessions and lifecycle | portable-pty, vt100/vte, parking_lot, nix (Unix), thiserror |
| nozhik-remote-ssh | SSHv2, protected credentials, host trust, SFTP, loopback forwarding | russh, russh-sftp, keyring, zeroize, tokio, serde/serde_json, directories, tempfile |
| nozhik-database | SQL pools, typed results, catalog metadata, background actor and owned SSH tunnels | sqlx, tokio/tokio-util, remote_ssh, keyring, serde/serde_json, tempfile |
| nozhik-ui | UI shell, documents, project/Git integration, editor view, search | core, buffer, config, event_bus, fs_model, vcs, plugin_host, language_support, terminal, remote_ssh, database |
| app | Инициализация, жизненный цикл, eframe | ui, config, event_bus |

Папка `crates/core` имеет имя пакета `nozhik-core`, чтобы не конфликтовать
с системным Rust crate `core`. Обратные зависимости и циклы запрещены.
Новые общие трейты добавляются в core при появлении реального потребителя;
абстрактного service locator и глобального singleton нет.

## Поток данных

1. `main` создаёт EventBus с ёмкостью 1024 и загружает ConfigManager.
2. До первой публикации регистрируется подписчик; Tokio runtime запускает
   потребитель на рабочем потоке.
3. Главный поток запускает eframe::run_native (egui + Glow). `NozhikUi`
   владеет документами и receiver событий; TextEditorView читает видимые Rope-строки.
4. Успешное создание окна публикует `ActionTriggered("app.started")`.
5. Изменение настроек: `replace` → validate → write lock → замена → release
   lock → `ConfigChanged(All)`. Подписчик получает новый snapshot.
6. Закрытие окна выходит из event loop. Все producers освобождаются;
   потребитель дочитывает очередь до `Closed`. На завершение отведены 2 секунды,
   затем runtime получает до 1 секунды на shutdown. Ошибки возвращаются из main.

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
видимые строки unified patch. Результаты worker будят egui callback-ом.

`FileEvent::Changed`, broadcast lag и успешный Save инвалидируют Git status с
200 ms debounce. Фоновый опрос раз в 3 секунды покрывает `.git` и внешние команды,
даже когда открыта другая IDE-панель. Changed-файлы Git определяет самостоятельно,
поэтому ignore-политика fs_model не скрывает tracked изменения из Git status.
Чекбоксы меняют реальный index; commit не перечитывает в него worktree автоматически.
Git работает с диском; буферы редактора сохраняются в памяти после branch/pull/discard.
Полные задачи, ответы и ограничения описаны в
[roadmap Phase 3.4](roadmap-progress.md#phase-34--vcs-engine--github-desktop-ui).

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

`DatabaseSchemaTree` → SchemaInfo → TableInfo { is_view, columns } → SchemaColumn
{ name, data_type, nullable, is_primary_key }. PostgreSQL использует information_schema
с join полного constraint identity, MySQL/MariaDB — information_schema с явным
UTF-8 CAST для бинарных catalog columns, SQLite — database_list/sqlite_master/
table_info. Идентификаторы в preview/PRAGMA цитируются по диалекту, catalog filters
используют bind parameters. Tree принадлежит UI как готовый snapshot.

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
История видна под кнопками Project и в hover submenu File → Recent Projects,
включая native macOS menu. При shutdown очередь записи дожидается ack до 1s.

Cut/Copy обрабатывают штатные egui clipboard
события, RequestPaste читает clipboard в winit; menu paste восстанавливает фокус
последнего активного редактора. Recent Files ограничен 30 путями.

`layout::Layout` хранит относительные left/right/bottom/sql размеры. Ordered worker
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
