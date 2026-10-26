# Работа с Nozhik

Перед изменениями читайте README.md, docs/ai/architecture.md,
docs/ai/coding-standards.md и docs/ai/roadmap-progress.md.

## Актуальные бинарники

Бинарники собираются локально и исключены из Git через `/binaries/`:
`binaries/macos-arm64/` и `binaries/windows-x64/`.
После каждой доработки обновляйте локальные бинарники ОБЕИХ платформ,
включая изменения build inputs (Cargo manifests/lockfile, Rust toolchain,
crates, .cargo, .gitattributes или scripts/update-binaries.py). Используйте `scripts/update-binaries.py`;
не подменяйте готовую сборку старым файлом с новой metadata.

На этом Mac доступны macOS ARM64 и Windows x64 GNU cross-build:

```sh
python3 scripts/update-binaries.py --target aarch64-apple-darwin
python3 scripts/update-binaries.py --target x86_64-pc-windows-gnu
python3 scripts/update-binaries.py --check
```

Для Windows cross-build нужны Rust target x86_64-pc-windows-gnu и MinGW-w64.
На Windows можно собрать x86_64-pc-windows-msvc. CI binaries.yml сохраняет сборки обеих платформ только как artifacts, без коммитов. Если обе
сборки локально невозможны, явно укажите недостающую платформу; не объявляйте
бинарники актуальными без успешного --check. Push выполняйте только в рамках
разрешённой пользователем задачи. Исходники нельзя считать проверенными только
по факту успешной сборки: запускайте проверки из coding-standards.md.
