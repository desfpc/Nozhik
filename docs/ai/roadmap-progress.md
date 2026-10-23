# Roadmap и прогресс

## Общий план

Этапы 1–9 — предварительная последовательность, не обещание готовых функций.

| Этап | Объём | Статус |
| --- | --- | --- |
| 0 | Каркас, документация, типы, шина, настройки, окно, CI | Реализован; проверки см. ниже |
| 1 | BufferStore, rope/piece table, версии, undo/redo, файловый I/O | Не начат |
| 2 | Рендерер редактора, ввод, выделения, прокрутка, Unicode | Не начат |
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

Локально на macOS arm64, Rust 1.99.1 stable: Clippy с pedantic/nursery и
`-D warnings` прошёл; все 5 unit-тестов и doc-tests прошли.
`cargo fmt --all -- --check` и release-сборка workspace также прошли.
Матрица CI предусмотрена для трёх ОС. Ручное открытие окна и проверка shutdown
на Windows, macOS и Linux остаются отдельными платформенными проверками.

## Вход в Phase 1

Нужны ревизии буферов, авторитетное хранилище текста и snapshot/resync API,
проверки UTF-8 границ, семантика line endings, политика владения BufferId.
Без этих решений нельзя восстанавливать состояние по BufferModified после lag.
Нельзя использовать broadcast как единственный путь исполнения обязательных
редакторских команд. Выбор структуры текста подтвердить измерениями.
