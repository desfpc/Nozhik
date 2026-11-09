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
| nozhik-language-support | PHP/CSS/YAML incremental syntax, local formatting, PHP/CSS project indexes, PHP stdio LSP | tree-sitter/php/css/yaml, csscolorparser, lsp-types, tokio, ropey, serde_json, thiserror |
| nozhik-docker | Compose file discovery, YAML run markers, scoped Docker CLI worker, PHP linter JSON reports, PHPUnit console commands | tree-sitter/yaml, ignore, serde/serde_json, thiserror, Unix nix |
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

## Заметки Snippets

Библиотека принадлежит открытому проекту и лежит в `<корень>/snippets.json`.
Файл создаётся, когда пользователь открывает раздел, а не при каждом открытии
проекта. Отсутствие файла — пустая библиотека. Пустой или повреждённый файл не
перезаписывается: правка отключается, сообщение уходит в `ProblemLog`. Успешное
копирование и вставка в терминал остаются в строке статуса.

Контракт: `{ "snippets": [ { "id", "title", "body", "tags", "created_ms", "updated_ms" } ] }`.
Порядок в файле — порядок создания. Список на экране сортируется по `updated_ms`.
Один worker пишет файл атомарно (`NamedTempFile` в том же каталоге, `sync_all`,
`persist`) с debounce 250 мс. Своё сохранение, совпавшее с памятью, не затирает
несохранённую правку. Удаление файла с диска очищает память и не создаёт файл
заново до следующей правки пользователя.

Окно повторяет схему Git: activity bar остаётся, левый dock заменяется колонкой
тегов, центральная область — списком и редактором. Ширина dock хранится в
`layout.json` отдельно от дерева проекта. В шапке редактора только копирование,
правка тегов и «открыть в консоли». Удаление доступно из контекстного меню и
клавишей Delete вне текстового поля.

«Открыть в консоли» копирует текст и вставляет его в активную сессию терминала.
Если живой сессии нет, создаётся локальная в корне проекта. Однострочный текст
не дополняется Enter. Многострочный текст отправляется только в режиме
bracketed paste; пока оболочка его не включила, вставка ждёт до двух секунд и
при таймауте остаётся в буфере обмена. Управляющие символы, кроме табуляции и
перевода строки, и пакет больше 64 КиБ в терминал не отправляются.

## OpenAPI

Иконка файла выбирается по имени, без чтения содержимого: `openapi.json`,
`swagger.json` и те же основы с `.yaml`/`.yml`, а также суффиксы `.openapi.` и
`.swagger.` для этих расширений. Остальной JSON остаётся со значком `{}`.
Дерево и вкладка редактора рисуют одну зелёную круговую метку. YAML с таким
именем метку получает, но панель GUI для него не строится.

Переключатель JSON | GUI появляется у `.json`, если документ — OpenAPI 3.x или
Swagger 2.0, либо имя уже опознано как спецификация и в тексте есть ключ
`openapi`/`swagger`. Обычный JSON, включая Postman, остаётся редактором без
переключателя. Режим по умолчанию — JSON. GUI не меняет буфер. Сессия на путь
файла хранит режим, выбранный сервер, черновики параметров и тел, раскрытие
операций и значения авторизации. Токены только в памяти процесса. Пока правка
остаётся спецификацией, сессия переживает смену revision. Невалидный JSON, в
котором ещё узнаётся спецификация, оставляет переключатель и текст ошибки.
Документ, который перестал быть спецификацией, сессию сбрасывает.

Разбор терпимый: одна битая операция или неразрешённый локальный `$ref` становится
предупреждением, остальные операции остаются. Поддерживаются локальные JSON
Pointer (`~0`/`~1`), sibling-ключи поверх `$ref` в 3.1, стили query form/explode,
deepObject, spaceDelimited и pipeDelimited, тело JSON, form и multipart, если
текст черновика — JSON-объект. `webhooks` попадают в тег `webhooks` и вызываются
на выбранном сервере. Callbacks перечисляются по имени и не исполняются.
Глобальный `security` наследуется; пустой массив у операции его отменяет.
Применяется первая полностью заполненная группа требований. OAuth2 и OpenID
Connect не открывают браузер: вставляется готовый токен и уходит как Bearer.
Относительный server URL (в том числе OAS-значение `/`) при Execute сообщает,
что нужен абсолютный `http://` или `https://`.

Запрос выполняет отдельный поток со своим current-thread runtime Tokio. Клиент
reqwest: timeout 30 с, connect 10 с, не больше 10 редиректов, user-agent
`nozhik-openapi`, тело ответа до 512 КиБ. Пока вызов не завершён, окно
перерисовывается примерно каждые 100 мс. Отмена Try it out оставляет черновик и
не обрывает уже ушедший запрос: поздний ответ отбрасывается вместе с receiver.
Порядок ключей в примерах совпадает с файлом: у workspace `serde_json` включён
`preserve_order`. На раскрытой операции кнопка To HTTP Requests собирает тот же
черновик, что и Execute, и передаёт его в библиотеку HTTP Requests.

## HTTP Requests

Библиотека принадлежит открытому проекту и лежит в `<корень>/http_requests.json`.
Файл создаётся, когда пользователь открывает раздел, а не при каждом открытии
проекта. Отсутствие файла — пустая библиотека. Пустой или повреждённый файл не
перезаписывается: правка отключается, сообщение уходит в `ProblemLog`.

Контракт: `{ "requests": [ { "id", "name", "method", "url", "query", "headers", "body_kind", "body", "form", "tags", "created_ms", "updated_ms" } ] }`.
`query`, `headers` и `form` — строки `{ "name", "value", "enabled" }`.
`body_kind`: `none`, `json`, `text`, `form`, `multipart`. Порядок в файле —
порядок создания. Список на экране сортируется по `updated_ms`. Файл читается
с `deny_unknown_fields`. Один worker пишет его атомарно (`NamedTempFile` в том
же каталоге, `sync_all`, `persist`) с debounce 250 мс. Своё сохранение,
совпавшее с памятью, не затирает несохранённую правку. Удаление файла с диска
очищает память и не создаёт файл заново до следующей правки пользователя.

Окно повторяет Snippets: activity bar остаётся, левый dock — Library
(All requests, Untagged) и Tags, центр — список и построитель. Ширина dock
хранится в `layout.json` полем `http_requests` (по умолчанию 0.18). Методы:
GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS, TRACE. Query хранится отдельно
от URL и отправляется у методов с телом тоже. Строка с выключенным флажком
остаётся в файле, но не попадает в cURL и в вызов. Смена вида тела выставляет
Content-Type; правка этого заголовка меняет вид тела. Multipart при отправке
ставит границу `nozhikHttpBoundary` и не использует пользовательский Content-Type.

Каноническая модель — запрос. Окно cURL — проекция: правка построителя
перерисовывает команду в том же кадре, до виджета cURL. Разбор команды
заменяет метод, URL, query, заголовки и тело и сохраняет id, имя и теги.
Неразобранная команда показывает ошибку и не очищает построитель. Выключенные
строки есть только в построителе: правка cURL заменяет включённый набор.

Вызов идёт отдельным потоком со своим current-thread runtime Tokio. Клиент
reqwest: timeout 30 с, connect 10 с, не больше 10 редиректов, user-agent
`nozhik`, тело ответа до 512 КиБ. Клиент OpenAPI остаётся `nozhik-openapi`.
Пока раздел открыт, Undo, Redo и поиск не трогают буфер кода. Копирование без
фокуса в поле копирует текст cURL. Удаление — контекстное меню карточки и
Delete вне текстового поля. Send и Cmd+Enter отправляют выбранный запрос.
Абсолютный URL обязателен: `http://` или `https://`.

Пределы: 2000 запросов, имя 200 символов, URL 8 КиБ, тело 256 КиБ, 100 полей,
поле 8192 символа, 64 тега по 40 символов.

Экспорт в OpenAPI не читает диск с потока интерфейса. Если документ уже открыт,
правка идёт в его буфер и обычный Save. Иначе worker создаёт
`<корень>/openapi.json`, а при любой ошибке создания читает существующий файл.
Пустой текст становится минимальным OpenAPI 3.0.3. Запись заменяет операцию
того же пути и метода и оставляет соседние методы и ответы. У OpenAPI 3 пустой
или отсутствующий `servers` заполняется origin URL. У Swagger 2 `host`
заполняется, только если он пуст; `basePath` снимается с пути, если путь им
начинается. Несколько открытых спецификаций выбираются меню кнопки To OpenAPI.
Модуль `http_requests` не зависит от `openapi`: перенос операции собирается
на стороне OpenAPI и приходит готовой записью.

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
checks project generation/request IDs before installing replies. File rows stay
28 points. The strip above the commit form is a vertical splitter: dragging it
resizes the changes list and the commit form. Each pane keeps a stable scroll id
and shows a scrollbar when its content is wider or taller than the pane, so a
long path, summary, or description does not push the other pane off screen.
Status polling does not reset that scroll position. Summary and Description copy
the current selection with the standard copy shortcut. On macOS the native Edit
menu owns that accelerator, so the copy action reads the focused commit field
before the diff or the editor. An empty selection does not fall through to
another surface.
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

Project, Git, SSH и Database используют единый левый dock и
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
и внешние промежутки 2 px для Project, Remote, Database, редактора и нижнего
окна. Git, Snippets и HTTP Requests занимают свою колонку без внутреннего
отступа. Фон оболочки `theme::chrome` виден в промежутках; rails имеют ширину
36 px и векторные кнопки 28 px. `tool_icons` рисует смысловые контурные иконки
через egui Painter, без Unicode-глифов, внешних шрифтов или новых зависимостей.

Цвета всех окон читаются из `crates/ui/assets/color-schemes.json`.
`theme::apply_id` ставит активную схему на контекст egui в начале кадра.
Стандарт — New UI Dark (`dark`). Рядом лежат New UI Light и Catppuccin Mocha.
Выбор хранится в `settings.toml` (`theme`) и в Settings → General.

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
unsaved text. Startup loads versioned, root-bound index_php_v6.json, then reconciles
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
No submodule recursion is added. Shared project authentication now retains clone credentials for later fetch/push during that project session (v0.1.12).

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
фокус, сворачивание и persisted resize. С v0.1.38 Problems — журнал сессии,
см. «Уведомления и Problems».


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
наследуют license-file, а не SPDX Apache-2.0. В v0.1.19 условия обновлены до
Source-Available License 1.1: покупка на одного пользователя, до трёх активированных
устройств на лицензированный продукт, привязка к email и активация по ключу.
Механизм активации/продления пока не реализован; внедрение — после релизной версии.
До доступности механизма техническая активация не требуется; ограничения условий
покупки сохраняются. Подписка не введена; самостоятельная
сборка для себя и внутренней работы организации разрешена. Сторонние компоненты
сохраняют лицензии. CONTRIBUTING.md ссылается на права для коммерческих сборок
и перелицензирования вкладов без передачи авторства.

Будущий контракт описан в [paid-software-licensing.md](paid-software-licensing.md).
Его наличие не означает готовность серверного API, лаунчера или клиентских проверок.

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

## Git Changes: игнорирование директорий (v0.1.10)

`Task::IgnoreFolder(PathBuf)` принимает repository-relative директорию и на Git
worker добавляет в корневой `.gitignore` экранированный anchored pattern с `/`
на конце. Валидация общая с Ignore: запрещены absolute/traversal/.git и переводы
строк; `.gitignore` должен быть обычным файлом. Index не меняется.
UI использует стандартный egui submenu: клик по названию применяет ближайшую
папку файла, стрелка выбирает одного из предков от ближайшего к верхнему,
исключая корень. При multiselect папка относится к файлу под контекстным кликом.

## Генерация сообщения коммита (v0.1.39)

`Task::CommitContext` читает на serial Git worker полный список staged-файлов
HEAD→index, с rename detection. У каждого файла свой unified patch: не больше
8 KiB, общий бюджет patch-текста 512 KiB. Сначала укорачиваются самые большие
патчи, поэтому один большой файл не скрывает остальные. Бинарный файл остаётся
в списке без тела патча. Заголовки hunk сохраняются даже при усечении тела.
Файл больше 8 MiB Git относит к binary. Index перечитывается с диска; fingerprint
учитывает HEAD/ref и path/OID/mode/flags всех entries. Конфликты, Git operation
и пустой staged diff отклоняются. Чтение не меняет index и worktree.

`Ollama::commit_message` не отправляет сырой diff одним запросом. Локальная
модель сначала получает пачки до 4 файлов и около 6 KiB патча и возвращает
фактическую заметку на каждый путь. Если заметок больше 24 или они длиннее
12 000 символов, заметки одной директории сжимаются отдельным запросом; файлы
из разных корней не склеиваются вслепую. Итоговое название и описание пишутся
только по заметкам, списку путей и областям. Слишком общий черновик
переписывается один раз, и в повторный запрос не вставляется текст плохого
черновика. Если ответ копирует служебную инструкцию промпта или повторяет
название в описании, форма заполняется самими заметками: описание — список
заметок, название — короткая строка по областям или по единственной заметке.
Заметка, которая сама является служебной фразой, заменяется статусом и именами
hunk. Бинарный или пустой patch не занимает запрос модели.
Непрочитанный ответ модели заменяется заметкой со статусом и именами hunk и
не выбрасывает файл. Каждый `/api/generate` идёт с JSON schema, stream=false,
num_ctx=16384 и deadline 120s; stop tokens автокомплита не применяются.
Ответ ограничен 64 KiB. Автозагрузки модели нет. Кнопка показывает шаг.

`vcs_panel::commit_ai` владеет abort-on-drop HTTP task/oneshot на app runtime;
worker, шаги генерации и HTTP completion будят egui без polling repaint. Кнопка над формой активна
только при ai_enabled/model/runtime, staged files и свободном mutation gate.
Текст формы сохраняется в snapshot: ручные правки отменяют ответ. Любая Git mutation,
смена проекта/AI settings, Cancel и shutdown прерывают запрос. После HTTP-ответа
worker повторно читает CommitContext; fingerprint должен совпасть перед заполнением.
Поздние worker replies дополнительно проверяются generation/request ID. Неактивные
проекты дренируют эти состояния через background tick. Генерация не создаёт коммит.

## Git project settings and shared clone authentication (v0.1.12)

`vcs::connection` owns non-secret GitSettings/AuthProfile/AuthMethod, validation,
atomic local-config updates and URL/profile-bound session Authentication. The
existing clone Authentication and credential resolver are shared by all transport
callbacks. Existing repositories use `repo.config()` (including repository-local
credential helpers), clones use the system config before checkout exists.
Network callbacks restrict credentials to the exact requested fetch/push URL and
bound attempts; standard certificate/SSH host-key verification remains enabled.
Transport errors are sanitized and point to Settings → Git. Local commit identity
errors separately point to author name/email settings.

Tasks GitSettings, SaveGitSettings { expected, settings, authentication },
Authenticate and TestGitConnection run only on the serial Git worker. Settings
responses carry no secrets. Save compares the latest effective settings under the
standard common-dir config.lock and atomically replaces only the local config,
retaining unrelated keys/permissions. Stale/invalid/locked/symlink config errors
leave the original untouched. Author/helper overrides are written only when edited;
blank overrides inherit system settings. origin URL/pushurl and default fetch refspec
can be added/changed; refs/index/worktree remain intact and saving performs no network.
Test reads the candidate remote with no refs/config writes; changed credential helpers
must be saved first. Push permissions are checked by push, not by the fetch test.

Only nozhik.auth.method/username/keypath are persisted, with ordinary origin/user/
credential.helper keys in Git config. Tokens/passwords/passphrases stay in zeroizing
session memory. Blank secret fields retain a previous credential only when both
endpoints and AuthProfile still match. External endpoint/profile changes stop reuse.
No new OS keychain writes or plaintext secret persistence are added.

UI Settings → Git delegates to VcsPanel and uses the same git_auth::AuthFields as
Clone Repository. Explicit Save/Reload/Test preserve existing general settings auto-save.
Draft secrets are cleared on dialog close and successful save; session secrets live
until project worker shutdown. Successful clone stores non-secret preferences and
returns owned Authentication alongside the path. The workspace prepared-open message
carries it to the target project; VcsPanel queues Authenticate after Open, with no
cross-project singleton or secret logging. Cancel/failed open discards the credential.

## Safe Git transport diagnostics (v0.1.13)

`vcs::transport_error` maps libgit2 error code/class and allowlisted HTTP status
codes to fixed actionable messages shared by Clone/Test/Fetch/Push. It never echoes
raw remote errors, response bodies or URLs, preserving secret redaction even if a
server reflects authentication data. GitHub 403 points to Resource owner/repository
selection/Contents permissions and organization approval; 404 notes private resource
hiding, 401/auth errors distinguish rejected credentials, and TLS/SSH/network/proxy
errors have separate explanations. Missing explicit session tokens/passwords fail
before connecting; token edge whitespace is trimmed by the shared credential resolver.
Token USERNAME negotiation consistently uses x-access-token.

The connection test now retains RemoteConnection until advertised refs are read;
RAII performs disconnect. The ignored live diagnostic accepts URL and token only
on stdin, stores token in Zeroizing and prints fixed safe errors, never raw transport
messages. No credentials, user-specific URLs or response bodies are fixtures in code.


## Визуализация объектов БД (v0.1.15)

`database_panel/tree.rs` рисует дерево векторными glyphs без внешних icon assets:
24 logical px на строку, 19 px на уровень, font 14/12, palette референса.
Состояние раскрытия хранит egui CollapsingState, выделение — DatabaseUi.selected_object;
ID включают connection/schema/table/category/object. Rows обрабатывают click,
double-click, context menu и Left/Right для раскрытия сфокусированной группы.
Tree возвращает типизированные actions; соединение, Describe, retry и SQL-tab
действия остаются в прежнем worker integration. Connection actions перенесены
в контекстное меню; offline-профиль также показывает Connect внутри ветки.
Начиная с v0.1.21 каждый namespace имеет собственный узел и контекстное меню,
включая единственную БД с именем database.

SchemaColumn дополнен is_foreign_key и default_value. Признаки берутся из native
catalog (в SQLite foreign_key_list), а не из названий колонок; display_type сохраняет
native type/default expression и оформляет MySQL auto_increment как (auto increment).
MySQL key/FK/index annotations компактны, составные indexes сохраняют порядок
колонок и один UNIQUE suffix. Ленивое получение деталей и локальные ошибки сохранены.


## Пространственные значения и страницы таблиц (v0.1.16)

MySQL decoder выделяет GEOMETRY до общего строкового fallback и использует
unchecked Vec<u8> decode только для этого native type: SQLx не включает GEOMETRY
в compatibility list бинарных типов. DbValue::Geometry хранит внутренний payload
SRID (4 little-endian bytes) + WKB без потерь; Display распознаёт POINT обоих
порядков байт, другие формы имеют hex preview. Geometry учитывается в byte limit.
Table grid запрещает ручную правку GEOMETRY, включая NULL; остальные ячейки
редактируются штатно. DML predicates сравнивают CAST(column AS BINARY) с исходным payload (включая SRID), geometry literal
использует ST_GeomFromWKB, поэтому Submit других колонок сохраняет координаты.

connection::table_page_sql цитирует schema/table/PK, добавляет ORDER BY PK,
LIMIT page_size+1 и OFFSET. Размер ограничен 1..=5000; preview_sql использует 501.
TableEditor по умолчанию хранит offset=0/page_size=500. Lookahead удаляется до
создания TableChanges, has_more управляет Next; нижняя оценка не требует COUNT.
Footer закреплён под viewport, размеры 100/500/1000/5000, First/Previous/Next.
Номера строк учитывают offset. Pending page применяется только при успешном
Reply::Query; ошибка/отмена сохраняет прежние диапазон и snapshot. Busy/dirty/modal
блокируют переходы; Reload и успешный Apply перечитывают текущую страницу.
Без PK серверный порядок не гарантирован; OFFSET не является snapshot isolation
между страницами и может сдвигаться при внешних вставках/удалениях.


## Временные значения MySQL (v0.1.17)

mysql_value отдельно обрабатывает DATE/TIME/DATETIME/TIMESTAMP через unchecked
String decode. Это ограничено native temporal types и опирается на существующий
инвариант query: все строки MySQL приходят через raw_sql / COM_QUERY text protocol.
SQLx NaiveDateTime type compatibility допускает только DATETIME, поэтому общий
fallback приводил к ошибке на TIMESTAMP. Текстовое чтение сохраняет значение
часов текущей серверной сессии без присвоения часового пояса, дробную часть,
legacy zero dates и отрицательные/многосуточные TIME; общий null gate сохраняет
SQL NULL. DbValue::DateTime и генерация DML не меняются, PostgreSQL/SQLite
сохраняют прежнее декодирование. Binary-protocol query results здесь не используются.
Regression fixture поднимает временный localhost MySQL wire server, выдаёт native
field codes и проверяет полный Connection::query, включая NULL и edge values;
реальный сервер и credentials не нужны, timeout — 10s.


## WHERE / ORDER BY и сортировка таблиц (v0.1.18)

TableQuery (database::table_query) содержит predicate/order без ключевых слов.
page_sql строит один bounded SELECT с WHERE (predicate), ORDER BY order и
LIMIT page_size+1 OFFSET offset; пустой order не добавляет скрытую PK-сортировку.
Legacy connection::table_page_sql также не добавляет ORDER BY. Валидатор допускает
кавычки с удвоением, сбалансированные скобки и вложенные выражения/подзапросы,
отклоняет separators/comments вне кавычек и внешние clause escapes. Размер каждого
поля ограничен 16 KiB. Неоднозначные backslash escapes отклоняются без догадок
о текущем session SQL mode. Синтаксическую/типовую корректность выражений проверяет
сервер; это не полный SQL parser. Идентификаторы таблицы и стрелочной сортировки
цитируются по диалекту.

TableEditor разделяет where_input/order_input (черновики), query (условия
загруженного snapshot), pending_query/pending_page (текущий запрос). Enter/Apply,
очистка поля и header click создают TableAction::Query с offset=0. Reply::Query
коммитит условия и диапазон только при успехе; query_failed сбрасывает pending,
сохраняя snapshot, применённую сортировку и текст полей для исправления. Page,
Reload и Apply follow-up используют применённые условия, а не несохранённый draft.
Dirty/busy/modal/invalid gates блокируют изменение выборки. Поля имеют vector
filter/order icons; header arrow рисуется треугольниками без зависимости от font.
Цикл None→ASC→DESC→None меняет серверный ORDER BY одной колонки. Ручные простые
одноколоночные ORDER BY распознаются в стрелках, сложные выражения — нейтральные.
Результаты SQL-консоли сортируют загруженный snapshot локально (см. v0.1.20).


## SQL completion и редактируемые результаты (v0.1.20)

EditorLanguage.sql_catalog содержит snapshot схемы, namespace и диалект. SQL-ветка
TextEditorView использует штатные popup/keys/completion edits без PHP/LSP запросов.
ui::sql_support лексически разрешает FROM/JOIN/UPDATE/INTO и aliases, фильтрует
таблицы/колонки по префиксу и цитирует вставляемые имена по диалекту. database_editor
лениво запрашивает Describe для упомянутых relations через прежний bounded worker;
метаданные дерева и консоли общие. I/O на UI-потоке не добавлен.

SqlConsole хранит executed_sql отдельно от редактируемого буфера и pending_sql.
Каждый result имеет TableEditor с console_result=true: общий toolbar/grid/preview,
без WHERE/ORDER BY и серверной pagination, с локальной сортировкой snapshot.
Консервативный анализатор разрешает DML только для прямых непереименованных колонок
одной base table, с полным PK для частичной проекции; полный snapshot без PK
сохраняет прежнюю exactly-one-row transaction проверку. JOIN/CTE/агрегаты/views/
вычисления/multi-statement и неоднозначная default schema остаются read-only.
Read-only snapshot может стать редактируемым после запоздавшего Describe.

TableChanges.insert_defaults содержит точные column/literal равенства из AND
предикатов, исключая OR/NOT, выражения и конфликтующие значения. add_row заполняет
представленные поля; statements добавляет constraints для отсутствующих в проекции
полей INSERT. Значения проходят обычный dialect literal encoder, без вставки
фрагментов WHERE в DML. Исходный query/connection/schema закреплены за snapshot;
Submit/reload используют выполненный SQL. Dirty/preview/busy gates не дают затереть
правки Execute или переключением connection/schema. Close/Save All не коммитят DML.


## Контекстные действия схемы и встроенный экспорт (v0.1.21)

`database::schema` содержит Settings/Grant, validation + modification_sql,
Task/Output и Artifact (Arc с owned NamedTempFile). Command::SchemaTask использует
существующие bounded queues, connection generation и cancellation tokens; schema
операции не получают credentials и не создают сторонних процессов. SQL, каталоги,
чтение scripts и запись export идут на worker; атомарная финальная запись —
spawn_blocking с проверкой cancellation перед persist. Пароли не дублируются.

MySQL/MariaDB dump использует одну checked-out connection и consistent-read
transaction: SHOW CREATE DATABASE/TABLE/VIEW/SEQUENCE/FUNCTION/PROCEDURE/TRIGGER/EVENT,
серверные type-aware literals, BIT/blob hex, spatial WKB+SRID, точные decimal и
временные значения в UTC. FK checks выключены при восстановлении; view placeholders
разрешают зависимости, программы сохраняют SQL_MODE, triggers/events следуют данным.
Sequence export сохраняет persisted high-water mark (возможны gaps из-за cache).
PostgreSQL dump использует repeatable-read/read-only snapshot, SET LOCAL search_path,
UTC/ISO, row_security=off (недостаточные права дают ошибку, а не filtered backup).
Catalog DDL включает обычные/partitioned/inherited tables, identity/serial sequence
settings/state, enum/domain/composite types, functions/procedures, views/mviews,
constraints/indexes, triggers/rules/RLS policies, collations/statistics и metadata.
Server quote_nullable(value) сериализует data без потери arrays/bytea/numeric/extension
values; data читаются FROM ONLY каждой физической таблицы, без дублирования partitions.
Неподдерживаемые native base/range types, aggregates, foreign tables, operators,
operator families/classes, conversions и text-search objects явно отклоняются.
Extension members восстанавливаются CREATE EXTENSION; внешние schema/server/role
зависимости экспортом одной схемы не создаются. SQLite snapshot хранит обычные
таблицы/generated exclusions, sqlite_master и sqlite_sequence, file pragmas.

CSV — одна выбранная таблица, engine text output (PG/MySQL), typed SQLite values;
UTF-8, RFC-style quoting/CRLF, comma/semicolon/tab/pipe, NULL marker, optional headers,
generated columns и bounded transpose. Экспорт потоковый, лимиты QueryResult не
применяются. Artifact хранит полный файл и prefix ≤64 KiB для UI; clipboard ≤32 MiB,
transpose ≤16 MiB, script file ≤64 MiB. Диалог выбора результата использует NSSavePanel
на macOS и rfd на других платформах; имя/путь назначает пользователь.

`database_panel/schema.rs` коррелирует ответы по request ID, блокирует поля во время
работы, хранит immutable preview artifact и сбрасывает его при смене параметров.
Drop диалога отменяет token и удаляет временный файл после завершения владельцев.
Modify повторно сравнивает исходный snapshot настроек перед применением, SQL preview
строится тем же modification_sql, что и execution. После Apply/Run обновляется catalog.
RunFile применяет schema на той же connection и drains все raw SQL results без row cap;
MySQL DELIMITER обрабатывается вне строк/комментариев, PostgreSQL dollar bodies
передаются SQLx без splitting. SQLite progress cancellation обязательно завершается
cleanup/ROLLBACK; экспорт/settings сбрасывают старый progress handler при новом acquire.
Server connections после scripts/dumps retire, чтобы не переносить USE/search_path/
SQL_MODE/transaction state между вкладками. Cancel не откатывает committed DDL/DML.


### Table designer (v0.1.22)

`database::table_design` owns dialect-aware Definition/Element snapshots, native
catalog parsing and migration_sql shared by preview and execution. LoadTable,
ModifyTable and TableDdl use existing SchemaTask queues and cancellation. Apply
rechecks source metadata; SQLite additionally rechecks under BEGIN IMMEDIATE.
PostgreSQL changes run in a transaction; MySQL combines compatible ALTER clauses
but DDL commits remain server-defined. SQLite rebuild preserves data/rowids/counters,
checks FKs and dependent views, and restores session pragmas even on cancellation.
Do not roll back an existing user transaction when our BEGIN did not succeed.
Native unmodified clauses are retained; no implicit CASCADE or external utilities.

UI designer keeps original/draft and stable monotonically allocated element IDs.
Left tree, right selected form and bottom SQL preview are bounded child regions;
worker results use request IDs and modal/project gates. Successful mutations
invalidate metadata and reload clean table tabs; failure invalidates potentially
partial results. Table export reuses Artifact/options with explicit only_table,
filtering catalogs before unrelated object checks; schema exports keep full scope.

Opening a table allocates its BufferId before Describe. Metadata/data loading are
separate states; replies update only the still-existing matching buffer, preserving
focus. Closing a read-only pending tab cancels work without a dirty confirmation.
Metadata errors clear loading and permit retry; catalog refresh restarts pending
requests against fresh metadata without reopening closed tabs.


## CSS services and class index (v0.1.23)

config::languages registers Css for case-insensitive .css extensions; disabled CSS
starts neither parser nor index/cache I/O. LanguageService routes CSS Sync/Format/Close
only to the syntax worker, with an independent incremental Tree-sitter CSS parser.
CSS never starts the PHP LSP or uses PHP stubs. Highlights include revision-bound
ColorOccurrence values, computed on that worker. csscolorparser handles literal
color conversion; tree-sitter-css and csscolorparser retain their MIT / MIT OR
Apache-2.0 upstream terms and generated distribution notices.

css::format edits whitespace between tokens and validates both the resulting AST
and exact token strings. Original UTF-8 selection boundaries and CRLF are retained.
The builtin css_default.json profile uses the existing CodeStyle/settings worker;
format replies retain BufferId/revision/profile gates and one undo transaction.

ProjectIndexer owns a Worker per enabled PHP/CSS partition, each with its own
SymbolIndex, stamps, overlays and atomic cache (index_php_v6.json/index_css_v6.json).
FileSymbols.css_classes retains raw UTF-8 ranges and decoded, case-sensitive CSS
identifiers; SymbolIndex::css_classes exposes them with source paths, independently
of PHP declaration normalization/linking. CSS snapshot events do not replace PHP
snapshots. Invalidate removes the cache once before rebuilding all active partitions.
No HTML completion is introduced.

TextEditorView paints multiple swatches per visible row in a gutter reserved before
the text. css_colors::Picker owns buffer identity, revision, source range and exact
sRGBA bytes. Graphical/RGBA/Hex edits replace only the selected literal and update
the revision/range for subsequent edits. External changes/undo, switching buffers
or a modal gate close the picker. Comments, strings and url contents are excluded.
Unambiguous in-document var chains resolve to literal definition ranges; ambiguous
custom-property definitions, external variables and computed colors remain unresolved.
This is a static preview, without browser cascade/DOM/import evaluation.

## CSS completion and cross-language class lookup (v0.1.24)

FileSymbols contributes class_occurrences (ClassOccurrence with case-sensitive name,
ReferenceLocation, declaration flag) and class_attributes (literal attribute ranges).
CSS uses the existing Tree-sitter class extractor; PHP reuses its current tree to
retain actual markup text and mask expressions/strings, then scans HTML attributes.
Comments and raw script/style/textarea/title bodies are excluded. Static class tokens
next to dynamic PHP are retained only when separated by HTML ASCII whitespace;
entity-encoded tokens and runtime values are not guessed. No HTML/JS service is enabled.

ProjectIndexer publishes a worker-built immutable ClassIndex and merged overlay
revision gates after the enabled partitions have changed. Lookups include all CSS
selectors and PHP/PHTML static usages, preserve case and source ranges, and never
reparse other files. Cache schema 5 rebuilds previous caches once, so markup facts
are not missing after upgrade; unsaved facts are still excluded from disk. Disabling
CSS removes class navigation/completion even if PHP indexing stays enabled. New
language extractors can populate the same facts without frontend extension lists.

EditorLanguage receives the class snapshot only for its exact source revision.
Typing queues a class completion refresh; index arrival completes it only for the
same caret. Class contexts claim completion before PHP/LSP responses, with explicit
TextEdit ranges preserving hyphens, Unicode and neighboring class tokens. CSS
properties use a separate lexical completion context and bundled identifiers,
without invoking PHP or parsing ASTs on the UI thread. Existing ':' is retained.

Cmd/Ctrl+click hit tests exact class glyph ranges, without opening on modifier hover.
CSS selectors open all class occurrences; markup opens one declaration directly
or a declaration chooser. CSS lists use egui Modal and the shared UsagesPopup row/
filter/keyboard renderer. Standard properties have no class navigation. PHP usages
keep their existing window/optional viewport behavior.


## New table designer (v0.1.25)

The connected tables folder routes + New Table into the existing Modify Table modal.
A creation draft has no original snapshot; creation_sql validates and renders native
CREATE TABLE plus indexes/triggers/options/comments/grants using the same object
renderers as migration_sql. schema::Task::CreateTable executes that exact preview
on the database worker, without IF NOT EXISTS or destructive replacement. SQLite
and PostgreSQL use transactions; MySQL can retain partially committed DDL. Reload
after a creation attempt reads the target if it exists. Successful Apply installs
the catalog snapshot as the baseline for subsequent modifications and refreshes
the catalog without renaming or invalidating other open tables. Cancel drops the draft.


## YAML syntax and formatting (v0.1.26)

YAML/YML is an available opt-in language in config::languages. The syntax worker
uses tree-sitter-yaml (MIT), with key captures taking priority over scalar captures.
Sync/Format/Close stay on the syntax queue; no PHP LSP/stubs or YAML index partition
is created. Local formatting derives collection columns from the parsed tree,
normalizes only indentation/separator whitespace and retains scalar tokens.
Multiline scalar signatures retain indentation relative to their owning collection.
Both AST shape and scalar/token signatures are checked after applying selected edits
to the complete document. Partial structural selections fail atomically.
YAML has a shared two-space built-in profile for both suffixes; tabs are rejected.
Enter copies leading indentation; paste is verbatim to preserve scalar contents.
The existing BufferId/revision/style gates and single-transaction Undo apply.


## Docker Compose integration (v0.1.27)

`docker` is UI/runtime-independent. Its project worker discovers conventional Compose
filenames without following symlinks or scanning dependency/build caches. No language
index is created. Immutable Document snapshots yield AST-based services/key-line run
markers; UI gates them by path, BufferId and revision, independently of YAML opt-in.

`docker compose config --format json` evaluates the exact ordered file selection.
The worker retains only names/images, not service environment data. Standard bases
combine with conventional override; explicitly selected variants combine with base.
`docker ps --all` filters the evaluated Compose project label, then inspect verifies
every container's project/working-directory label against the canonical configuration
directory. Name collisions with foreign directories fail before mutations. IDs are
validated hex and used as direct arguments; no shell interpolation is involved.

Snapshot/Up/Start/Stop/StopAll/Logs/Console requests have UI tokens. CLI child stdout
and stderr are drained concurrently with bounded retained output; commands have
timeouts and cancellation. Cancel/shutdown terminate the owned CLI process tree (including Compose plugins), never implicitly stop
containers. Discovery works without invoking Docker; engine queries run only while
the Docker view is visible or after explicit actions. The UI disallows actions with
unsaved configuration files. The toolbar always includes Cancel (enabled only while
busy); only user-initiated requests show a spinner in a fixed toolbar slot.
Periodic snapshot/log refreshes remain silent and do not shift the list. Logs retain the last 500 timestamped stdout/stderr lines.

The Docker tool appears between Database and Tools and requires a nonempty
Compose inventory. Editor/tree tabs use a native vector whale icon. Container Info
shows immutable inspect snapshots; Logs refresh while visible. Console replies pass
a validated `docker exec -it -- ID /bin/sh` invocation to TerminalPanel's existing
PTY SessionOptions/ShellCommand, with a `[Docker: name]` tab. Docker console tabs
retain exited output (including missing-shell errors); other terminal behavior is
unchanged. Each NozhikUi project owns its own worker/selection and CLI cancellation.

### Project tree context actions (v0.1.28)

Tree row context menus queue filesystem commands; CreateEntry validates a single
portable name and canonical project parent on the serial worker, then uses exclusive
create. EntryCreated/EntryCreationError are separate from save errors. Success
closes the project-local modal, reloads the parent and opens new files through the
existing editor read path. A refresh error after creation cannot retry the mutation.
ShowInFileManager runs direct OS argv on the worker (macOS open -R, Windows
Explorer /select, Linux xdg-open of the parent); no shell or new dependency.

### Index memory and mailboxes (v0.1.30)

ReferenceLocation stores a shared Arc<str> excerpt of at most 512 UTF-8 bytes,
plus preview_offset independent of the original byte/line/column coordinates.
PHP and CSS use the same bounded extraction; usages highlighting uses the excerpt
offset and navigation retains original positions. SymbolIndex shares immutable
FileSymbols via Arc. Worker disk/live and UI snapshots share Arc<SymbolIndex>
when no overlay diverges; mutation uses copy-on-write. The UI no longer retains
an unused CSS SymbolIndex alongside its class catalog.

ProjectIndexer commands coalesce Overlay/Close per path, retain independent disk
changes and replace more than 256 pending distinct Changed paths with Reconcile.
No file notification is silently discarded: Reconcile checks all source metadata.
A batch publishes at most one snapshot per changed partition and one class catalog.
The event mailbox retains the latest event per kind, with Progress/Finished sharing
one slot; obsolete snapshots are released before UI drains. Drop clears pending
commands and wakes the sleeping worker. Schema 6 uses new cache filenames, so legacy
huge JSON is never deserialized. Old per-language files are removed only after a
successful atomic replacement of the new cache.

Saving documents is independent of index memory. Undo already retains copy-on-write
Rope snapshots and edits, not complete independent file copies per keystroke. Git
tracks staged/committed/worktree states and cannot replace editor Undo. This change
preserves explicit save and dirty/revision gates; per-keystroke disk writes would
also trigger filesystem/Git work and split typing undo groups via mark_saved.


### Local LLM console (v0.1.31)

NozhikUi owns a separate optional LlmPanel per project. It composes TerminalPanel
in agent-monitoring mode, preserving ordinary terminal/SSH/Docker behavior and
independent tabs. Lazy startup, hide/reopen, inactive-project ticking, clipboard
routing and bounded terminal shutdown use the existing PTY lifecycle.

terminal::agent identifies exact Codex/Grok executables using bounded background
ps queries. Unix checks PTY foreground identity; macOS queries the foreground
group to support npm/node wrappers. Other Unix platforms currently recognize a
direct foreground executable. Windows monitoring is not wired; its LLM panel
retains terminal input without assisted-context controls. No output/title matching,
remote discovery, MCP server, model API or credentials are introduced.

SessionSnapshot carries an optional ForegroundAgent (type + native PID).
AgentPaste passes through the control worker (foreground identity and bracketed
paste check), then the writer rechecks the selected process's foreground/group
identity after earlier input batches. Context batches are atomic <=64 KiB with
explicit bracket delimiters, no Enter and rejected terminal control characters.
External transports reject assisted paste. Worker errors follow TerminalEvent.

LlmPanel's bounded preparation mailbox receives immutable Rope snapshots or local
paths. Its worker owns a private TempDir, stages file attachments with ASCII names
and serializes editor snapshots without modifying source files. Open-file context
maps originals/revisions to snapshots; selection includes 1-based UTF-8 byte
coordinates and half-open byte ranges. UI excludes SFTP and binary documents.
Results retain terminal ID and agent PID/type; switching tabs or replacing the
agent rejects stale work. File preparation/storage are bounded; lifetime is the
project panel, surviving hide/reopen and ending after terminal shutdown. Image
paths use CLI-native bracketed-paste attachment recognition, verified for Codex;
Grok's same route is documented in upstream paste.rs. Native model/file-format
support remains the responsibility of the installed CLI.


### Terminal mouse, frozen selection and glyph fallback (v0.1.32)

TerminalGrid publishes negotiated xterm mouse mode/encoding, wrapped-row flags,
normal history length, absolute viewport start and a selection epoch. The UI
encodes ordered native press/release/motion and wheel events only on the active,
enabled terminal surface. Shift overrides application capture. SGR coordinates
support the full 500-column viewport; legacy X10 and UTF-8 bounds are enforced.
Trackpad fractions and pointer capture are session-local. Fullscreen programs own
their scrollback; without mouse negotiation wheel falls back to cursor keys.

Emulator lazily clones its vt100 Screen for local review. Live parser/read/write
continue independently while the worker presents frozen cells. No complete history
is cloned during ordinary output. Text selection uses absolute frozen-buffer rows,
so wheel/scrollbar/page navigation do not reset the anchor. CopyText is a bounded
worker command guarded by epoch, joins soft wraps, and emits TerminalEvent::Copied.
Normal scrollback retains at most 3000 rows; an alternate screen contributes only
its visible buffer. Native TUI selection remains available for its internal history.
Returning live or resizing discards the frozen clone and invalidates stale ranges.

Native mouse input opens a 5-second OSC 52 copy window. Callbacks decode UTF-8
using the existing locked base64 package (now a direct terminal dependency), cap
text at 4 MiB, and retain at most one pending write. Workers publish Copied outside
shared locks; TerminalPanel applies the requested OS clipboard write. No clipboard
read query is answered and unsolicited OSC 52 writes are dropped.

A dedicated Terminal family uses embedded unmodified JetBrains Mono, Noto Sans
Symbols 2 and Noto Emoji, followed by stock egui fallbacks. Glyphs are fitted and
clipped to one/two terminal cells; cell text and copy remain unchanged Unicode.
OFL notices are retained with the assets and added to generated legal sidecars.


### Project SFTP Deployment (v0.1.33)

Config has an optional SftpDeployment {ssh_profile, remote_root}, validated and
persisted through the existing project settings worker. Settings adds a category
with one saved-project SSH profile, absolute POSIX root, explicit Apply and Disable.
Draft invalid paths never replace a working config. No credentials are duplicated.
TreeActions conditionally exposes SFTP Deploy for files/directories/root; dirty
local buffers under the selection must be saved before a request can start.

DeploymentUi binds an increasing request ID to project root, selection, config and
SSH identity. RemoteWorker's Deployment intent reuses Verify/Trust/credentials/
Connect and returns a cloned authenticated SshSession. Typed DeploymentFailed replies
do not affect editor save completions. Cancelled/stale authentication replies are
ignored; accepting a session rechecks settings/identity/root and the dirty gate.
An active transfer keeps its original destination instead of following config edits.

A dedicated cancellable worker builds DeploymentPlan off the UI thread, excluding
.git/.nozhik and symlinks, checking containment/UTF-8 paths and bounding traversal to
100000 entries. SftpClient deploys project-relative paths, creates empty/missing
directories, resolves the configured remote root, rejects internal remote symlinks
and streams binary files in 64 KiB chunks with per-operation deadlines. Existing
files are truncated/overwritten; remote extras are retained. Cancellation/failure
can leave the current file partial and earlier files uploaded; no rollback or
atomic-project-publication guarantee is made.

A latest-only shared progress snapshot wakes egui at most every 100 ms plus completion.
Cancellation is checked during traversal and raced against async transfer, with
bounded shutdown before the project's SSH worker. The active and inactive project
loops both drain deployment completion. Transfers use separate SFTP subsystems on
the shared transport and are outside the serialized SSH worker's 35-second deadline.
Remote browser cache is not automatically refreshed by this independent subsystem;
use its Refresh after a deployment when browsing the same server directory.
No new dependency, remote shell command or external deployment tool is introduced.


### SFTP Deploy из Git History (v0.1.34)

VcsPanel получает availability/busy deployment, не SSH-профиль. Toolbar History
содержит SFTP Deploy Files и постоянное пояснение current working files. Запрос
доступен только для одного выбранного SHA с загруженными соответствующими details;
pending details/mutation, multi-selection и transfer блокируют кнопку. Клик
снимет список HistorySelection.files относительно canonical repo.root; UI отдаёт
его NozhikUi как одноразовый запрос. Git blobs не извлекаются, Git не изменяется.

Deployment worker принимает Sources::Path либо Sources::Files. Второй вариант
использует DeploymentPlan::files: только точные текущие regular files в canonical
project root, без рекурсивного раскрытия директорий; dedup, skipped missing/
internal/symlink/directory/outside-project paths и общий лимит 100000 сохраняются.
Dirty gate проверяет только запрошенные файлы, повторно после SSH auth. Upload
читает дисковые bytes через общий SFTP streamer. При отсутствии подходящих файлов
возвращается ошибка до удалённых записей. Прогресс/отмена доступны также в Git.
Remote actions dropdown использует square 36×50 chrome рядом с Pull/Push без
рамки и межкнопочного разделителя, сохраняя popup и прежние Git actions.


### JSON и Tools (v0.1.35)

LanguageId::Json регистрирует .json без учёта регистра. Syntax worker использует
language_support::json: UTF-8 lexer и serde IgnoredAny validation; JSON не передаётся
PHP actor и не входит в список PHP/CSS index workers. JSON formatting через прежний
Command::Format сохраняет BufferId/revision/profile gates и атомарный Undo.
Форматтер меняет только промежутки между токенами, не пересериализует Value:
порядок/дубли ключей, строковые escapes и числовые lexemes сохраняются.

ui::tools принадлежит сессии проекта. LeftTool::Tools исключает боковой dock и
занимает CentralPanel вместо файлового редактора, как Git center. Внутри — каталог
либо выбранный JSON; Back to Tools возвращает каталог, переключение rail сохраняет
scratch text. Плавающих Window для Tools/JSON нет. Minified JSON находится сверху,
Formatted JSON снизу; drag splitter меняет соотношение высот. TextEdit размещается
через show в top-down/Align::Min layout, без add_sized (centered overflow обрезает
начало длинных строк). ScrollArea ограничивает обе оси каждой панели независимо.
При переносе файла сбрасываются cursor/selection/undo и scroll offsets к началу.

Open in Tools / JSON копирует текущий buffer snapshot. Отдельный worker с bounded
input queue форматирует оба представления, возвращает spans и будит UI; revision
gate отклоняет stale replies. Незавершённый ввод сохраняется до валидного результата.
Cursor/selection remap следует позиции внутри токена, включая Unicode/пробелы в
строках. Лимит 2 MiB; clipboard и scratch content не пишутся на диск, переключение
раздела сохраняет текст до закрытия проекта. Новых зависимостей/semantic JSON cache нет.

### Миграция языков (v0.1.36)

Config.language_catalog_version=1 маркирует сохранённую политику с JSON. Отсутствующий
маркер десериализуется как 0; ConfigManager::read после успешного разбора добавляет
JSON в непустой legacy enabled_languages и устанавливает маркер 1 в памяти. Пустой
список остаётся пустым. Миграция не пишет файл; при обычном последующем сохранении
маркер позволяет сохранять явный opt-out JSON. Остальные языки не изменяются.


### URL / Base64 scratch tools (v0.1.37)

ui::tools now owns Option<ToolKind> selection and three independent ToolEditor
instances for JSON, URL and Base64. Each owns its two texts, spans, split ratio,
revision, last-edited side, mode, bounded worker queue and pending input. Editor
and ScrollArea IDs include ToolKind so focus, selection, undo and scroll state
cannot leak between instruments. The JSON file-transfer action selects only JSON.
The shared stacked UI retains top-left overflow and the full-size CentralPanel.

Only JSON normalizes both source/destination and remaps token-relative cursors.
Codecs preserve the edited source bytes/cursor and replace only the opposite
representation. Background jobs carry URL mode and revision; stale results cannot
replace newer input. Invalid escapes/Base64/non-UTF-8 leave edited input and last
valid opposite text intact. Clear invalidates pending results for its own editor.

URL conversion is strict UTF-8 component percent encoding (RFC 3986 unreserved)
or form-urlencoded via the explicit mode checkbox (spaces/+ and * versus ~).
Decoded '%' sequences require two hex digits; '+' is literal outside form mode.
Base64 reuses workspace base64 0.22.1 (already shipped through terminal), with the
standard alphabet/padded encoding. Decode accepts optional padding and ASCII
whitespace but rejects invalid trailing bits, characters and non-UTF-8 output.
No new packages, network calls, disk writes or language/index registrations.

## Уведомления и Problems (v0.1.38)

Кнопка **Problems** внизу левой полосы открывает нижний dock `! Problems`.
Внутри одна раскрывающаяся вкладка категории. Сейчас есть только **Project** —
общие проблемы сессии, в том числе настройки: неверная авторизация Git, отказ
базы данных, SSH, SFTP, Docker, файловый watcher, форматтер, индексация,
терминал и сервис AI. На вкладке цифра — число проблем этой категории.
Раскрытие показывает список карточек. Другие категории можно добавить в
`ProblemCategory` позже, не меняя журнал.

Журнал живёт в `NozhikUi` одного проекта и не сохраняется. Повтор той же
категории, источника, заголовка и текста записывается один раз и не показывает
toast снова. Лимит — 100 записей; при переполнении уходит самая старая.
Запись не исчезает, когда поле ошибки очистилось: это история сессии.
«Connection test succeeded» проблемой не считается.

Новая проблема появляется справа снизу. Одновременно видны три последних
уведомления; более ранние ждут в полосе «N more» и не стареют, пока не выйдут
на экран. Из видимых более старое выше, новое ближе к правому нижнему углу.
Карточка держится 3 секунды и за 0,5 секунды плавно исчезает. Наведение
останавливает таймер. Крестик в правом верхнем углу закрывает карточку за
0,2 секунды. Закрытие toast не удаляет проблему из списка.

Цвет зависит от серьёзности. Info — тёмно-серая карточка и синий круг с «i».
Warning — тёплая карточка и оранжевый «!»: отключение языка или форматирования,
обрыв SSH, отмена деплоя, бинарный файл, пропуск уведомлений шины, занятый
Docker. Error — красноватая карточка. Critical — красная заливка: смена host
key и остановка или исчерпание worker/очереди/идентификаторов.

Строка статуса не рисует текст, который совпадает с текстом проблемы или
заканчивается им (не короче 8 символов). Само поле `status` сохраняется.
Счётчики LSP-диагностик в статусе остаются: это будущая вкладка редактора, не
Project. Красные подписи ошибок убраны из панелей открытого проекта. На месте
остаются ошибка клонирования, история проектов до открытия папки, текст
диалога host key, предупреждение проверки в Tools и успешные подписи.

Пока toast виден, кадры запрашиваются через `request_repaint_after`: 16 мс во
время затухания и не чаще чем до следующего шага удержания. Безусловного
repaint нет. Новых зависимостей нет.

## PHP-линтеры в Docker (v0.1.49)

Проект хранит список `Config.linters`: по одной записи на `phpstan`, `phpcs`,
`psalm` и `phpmd`. В записи — id и имя контейнера, путь к бинарю внутри
контейнера, рабочий каталог и аргументы. Пустой файл настроек по-прежнему
означает пустой список. Ручная правка в Settings → Linters не перезаписывается
автообнаружением.

Кнопка с жучком на верхней панели запускает все настроенные линтеры текущего
проекта. Пока список пуст, кнопка неактивна. Команда идёт через `docker exec -i`
(`-w`, когда рабочий каталог задан) отдельным потоком, не через Compose worker:
линтер может работать минуты и завершается с ненулевым кодом, когда находит
замечания. `process::capture` сохраняет stdout в этом случае. Формат вывода
принудительно JSON. Невалидный JSON и фатальная ошибка конфигурации становятся
записью Problems и не роняют интерфейс.

Путь из отчёта склеивается с корнем проекта, если он относительный. Абсолютный
путь сначала отрезает рабочий каталог контейнера, затем самый длинный подходящий
mount. Вкладка линтера в `! Problems` группирует строки по файлам; клик открывает
файл и ставит курсор. В редакторе строка получает метку на полях и подчёркивание.
Панель агента вставляет тот же список в черновик заголовком
`### Linter Diagnostics Context:`.

Если линтер ещё не записан в настройках, ввод команды в терминале контейнера
или `docker exec` запоминает бинарь, аргументы и контейнер. Уже сохранённый вид
линтера не меняется. Оригинальные байты команды в PTY не подменяются.

## PHPUnit в консоли Docker (v0.1.50)

Кнопка запуска на верхней панели открывает отдельную консоль на каждый
настроенный тест включённого языка. Сейчас это только `PHPUnit`: вкладка
`[Docker: контейнер · PHPUnit]` выполняет `docker exec -it` с сохранёнными
бинарём, рабочим каталогом и аргументами. IDE не разбирает вывод.

`Config.tests` хранит язык, вид, контейнер, бинарь, каталог и аргументы.
Пустые настройки остаются пустым списком. Settings → Linters и Settings → Tests
начинаются с переключателя языков, включённых в Settings → Languages. У PHP
показаны текущие линтеры и форма PHPUnit. У остальных включённых языков обе
страницы пустые, пока для них нет раннера.

Команда `phpunit` или `vendor/bin/phpunit` в терминале контейнера запоминается,
если PHPUnit ещё не записан. Уже сохранённая команда не меняется. `--version`
и `--help` регистрируют бинарь без аргументов. Язык, выключенный в Languages,
не запускается кнопкой.
