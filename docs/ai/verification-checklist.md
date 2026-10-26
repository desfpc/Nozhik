# Проверка релиза Nozhik v0.1.0-RC1

RC1 = Feature Complete описанного набора Editor/Git/PHP LSP/Terminal/SSH/SQL.
Незаполненный gate требует выполнения, а не предполагается успешным по наличию CI.
Дата текущей проверки: 2026-10-26. Используйте временный проект и отдельную конфигурацию.

## Автоматизированные gates

- [x] `cargo fmt --all -- --check`.
- [x] `cargo clippy --workspace --all-targets --all-features --offline -- -D warnings` на macOS ARM64.
- [x] Тот же Clippy с `--target x86_64-pc-windows-gnu` (cross compile).
- [x] `cargo test --workspace --all-features --offline` вне sandbox: 87 unit + 8 integration + 1 doc-test = 96 passed; 4 system tests ignored (PTY, SSH localhost, FSEvents включены).
- [x] `cargo build --workspace --release --offline` на macOS ARM64.
- [x] Lifecycle tests: несохранённый текст, устаревший save ack, ошибка записи блокирует выход, Discard освобождает буфер.
- [x] Menu catalog IDs зарегистрированы; текущий keymap используется меню и actions search.
- [x] Drag левого splitter через egui pointer events → запись/чтение layout.json.
- [x] Malformed layout.json не перезаписывается; нечисловые/некорректные пропорции отклоняются.
- [x] Idle frame tests: пустой workspace и скрытый non-Git проект не назначают бесконечный zero-delay repaint.
- [x] `cargo test -p nozhik-ui --test stress_tests -- --nocapture`: 10 МБ Rope / 100k строк, grid 100k×50 с вертикальной/горизонтальной прокруткой, PTY + Git + mock LSP.
- [x] macOS ARM64 и Windows x64 release artifacts обновлены скриптом и прошли `scripts/update-binaries.py --check` на финальных исходниках.
- [ ] GitHub CI matrix macOS/Windows/Linux успешно завершилась на RC commit.
- [ ] Нативные MSVC/ConPTY и Linux UI/runtime тесты выполнены на соответствующих ОС.

## Матрица фактически выполненного

| Проверка | macOS ARM64 | Windows x64 | Linux |
| --- | --- | --- | --- |
| Build + lint | Выполнено | GNU cross compile | Не запускалось |
| Unit/integration/stress runtime | Выполнено | Не запускалось | Не запускалось |
| Native menu | AX видит все группы и File availability | Embedded menu проверено компиляцией | Не запускалось |
| Визуальное UI/мышь на реальном дисплее | Частично; экран заблокирован, screenshot недоступен | Не выполнено | Не выполнено |
| Idle CPU | 0.6% одного ядра, 30.01s, screen locked | Не измерено | Не измерено |
| Реальные OS credential stores | Не проверялись в этой фазе | Не проверялись | Не проверялись |

## Ручная проверка каждой целевой ОС

Запуск с отдельной конфигурацией:

```sh
cargo run -p app -- --config /absolute/test/settings.toml /absolute/test/project
```

На Windows используйте абсолютные native пути. `--config` меняет путь IDE settings/layout/SSH/DB profiles;
глобальный каталог плагинов и keymap остаётся стандартным пользовательским.

- [ ] File/Edit/View/Navigate/Code/Git/Database/Terminal/Tools/Help доступны; disabled items соответствуют контексту.
- [ ] macOS: menu bar и Cmd accelerators; Windows/Linux: встроенное меню и Ctrl accelerators.
- [ ] Переопределить chord в keymap.json, проверить подпись меню и actual dispatch без двойного выполнения.
- [ ] New File не перезаписывает существующий путь; Open File/Folder различает тип без UI-блокировки.
- [ ] Save / Save All / Close Tab / Settings / Exit и shortcuts из README работают.
- [ ] Undo/Redo, Cut/Copy/Paste в редакторе, Copy в Data Grid; команды не изменяют соседний документ.
- [ ] Find → Next, Replace со строкой другой длины, Replace All → одно Undo; Unicode/CRLF сохраняются.
- [ ] Recent Files восстанавливает закрытый файл; закрытие неактивной вкладки не меняет активный документ.
- [ ] Растянуть left/right/bottom/SQL splitters мышью, перезапустить IDE, проверить пропорции и минимальную область редактора.
- [ ] Theme: единые цвета/радиусы/dividers/tooltip, 100%/150%/200% DPI, dark/light system appearance при фиксированной dark theme.
- [ ] Открыть файл 100 000 строк, прокрутить/редактировать; измерить FPS/frame time на реальном дисплее.
- [ ] Прокрутить большой Data Grid в обе стороны; сортировка/selection/copy не теряют соответствие строкам.
- [ ] Запустить реальный PHP LSP, терминал со streaming output и Git polling одновременно; UI остаётся отзывчивым.
- [ ] SQLite/PostgreSQL/MySQL/MariaDB: тест подключения, metadata tree, SQL, cancel, SSH tunnel через проверенный host key.
- [ ] Закрыть окно с dirty local/SFTP/SQL tabs: Cancel сохраняет окно, Save All ждёт записи, Discard не пишет изменения.
- [ ] Вызвать ошибку записи/отключить SSH во время Save All: окно остаётся, dirty и текст сохранены, ошибка видима.
- [ ] Проверить отсутствие дочерних LSP/shell после штатного выхода; SSH/tunnel sockets закрыты, SQL pool sessions освобождены.
- [ ] Проверить TOFU/changed host key и password opt-in в реальном Keychain/Credential Manager/Secret Service на тестовых credentials.
- [ ] Full screen, уменьшение/восстановление окна, modal keyboard focus, системный clipboard.

## Воспроизводимый idle/performance протокол

- [ ] Release build, отдельный пустой проект, без активного ввода/анимаций/запросов; подождать завершения стартовых I/O.
- [ ] На каждой ОС снять cumulative CPU time до/после интервала ≥30s.
- [ ] Вычислить `(CPU seconds delta / wall seconds) × 100`, где 100% — одно ядро; записать ОС, GPU, DPI, visible/minimized/locked.
- [ ] Повторить для unlocked foreground, minimized, пустого non-Git проекта, Git проекта и большого открытого файла.
- [ ] Цель idle ≤1% одного ядра; при превышении снять профиль repaint causes/threads и исправить источник.
- [ ] Отдельно измерить streaming workload, FPS и пиковую память; не применять idle-бюджет к активным задачам.

Локальный замер macOS: 0.18 CPU seconds / 30.01 wall seconds = 0.6% при locked screen.
Debug stress diagnostics: Rope edits + viewport ~16ms; synthetic 5M-cell grid + 10 layout passes ~97ms,
197 видимых text shapes. Эти замеры не заменяют FPS/foreground matrix и не гарантируют CPU на другом железе.

## Ограничения и блокирующие release gates

Runner/debugger, marketplace, подписанные/нотарифицированные installers, полноценный accessibility/security audit,
editable/paginated SQL grid, persistent SQL/terminal sessions и ранее описанные ограничения вне scope RC1.
Нативное UI тестирование Windows/Linux, foreground/DPI QA и реальный OS credential store остаются открытыми gates.
Системные ignored database/sshd/vi tests требуют отдельного запуска в подготовленном тестовом окружении;
в этой фазе старые system tests повторно не заявляются выполненными.
