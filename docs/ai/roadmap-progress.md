# Roadmap и прогресс

## Общий план

Этапы 1–9 — предварительная последовательность, не обещание готовых функций.

| Этап | Объём | Статус |
| --- | --- | --- |
| 0 | Каркас, документация, типы, шина, настройки, окно, CI | Реализован; проверки см. ниже |
| 1 | Текстовое ядро: Rope, версии, курсоры, транзакции, история, поиск | Реализован |
| 2 | UI Shell & Windowing: egui, панели, редактор, поиск | Реализован; ограничения ниже |
| 3 | Проекты, VFS, файловое дерево, watcher, поиск | Не начат |
| 4 | Tree-sitter, подсветка, синтаксические индексы | Не начат |
| 5 | LSP, диагностика, completion, переходы и рефакторинг | Не начат |
| 6 | БД: подключения, схема, SQL-консоль, результаты | Не начат |
| 7 | Git, terminal, task runner, запуск и отладка | Не начат |
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

Границы этапа: демонстрационное дерево и in-memory документы; реального VFS,
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
