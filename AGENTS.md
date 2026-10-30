# Работа с Nozhik

Перед изменениями читайте README.md, docs/ai/architecture.md,
docs/ai/coding-standards.md и docs/ai/roadmap-progress.md.

## Актуальные бинарники

Бинарники собираются локально и исключены из Git через `/binaries/`:
`binaries/macos-arm64/` и `binaries/windows-x64/`.
По умолчанию после каждой доработки обновляйте локальные бинарники только
текущей платформы, включая изменения build inputs (Cargo manifests/lockfile,
Rust toolchain, crates, .cargo, .gitattributes или scripts/update-binaries.py).
На этом Mac обычная сборка — macOS ARM64. Windows cross-build и Windows Clippy
локально запускайте только по явному запросу пользователя; не создавайте
Windows-кэш в target при обычной работе. Используйте scripts/update-binaries.py;
не подменяйте готовую сборку старым файлом с новой metadata.

Обычные локальные команды (сборка и проверка текущей платформы):

```sh
python3 scripts/update-binaries.py
python3 scripts/update-binaries.py --check
```

Windows по необходимости на этом Mac:

```sh
python3 scripts/update-binaries.py --target x86_64-pc-windows-gnu
python3 scripts/update-binaries.py --check --platform windows-x64
# Удалить Windows-кэш после сборки, сохранив готовый бинарник в binaries/:
rm -rf target/x86_64-pc-windows-gnu
```

Для Windows cross-build нужны Rust target x86_64-pc-windows-gnu и MinGW-w64.
На Windows можно собрать x86_64-pc-windows-msvc. Проверка всех комплектов
доступна явно: `python3 scripts/update-binaries.py --check --all-platforms`;
Windows-бинарник между явными сборками может отставать от исходников.
Не объявляйте его актуальным без успешной проверки этой платформы.
CI binaries.yml сохраняет сборки обеих платформ только как artifacts, без
коммитов; CI не занимает локальное место. Push выполняйте только в рамках
разрешённой пользователем задачи. Исходники нельзя считать проверенными только
по факту успешной сборки: запускайте проверки из coding-standards.md.

## Версия приложения

Версия ведётся в `[workspace.package].version` корневого `Cargo.toml`.
Каждая следующая доработка/коммит увеличивает patch-версию один раз и обновляет
`Cargo.lock`; интерфейс и app bundle используют эту же версию.
Текущая версия — 0.1.9 (ленивые метаданные БД и редактирование подключений), следующая — 0.1.10.
