# Готовые сборки Nozhik

| Платформа | Скачать | Метаданные |
| --- | --- | --- |
| macOS Apple Silicon (ARM64) | [nozhik](https://github.com/desfpc/Nozhik/raw/refs/heads/master/binaries/macos-arm64/nozhik) | [build.json](macos-arm64/build.json) |
| Windows x64 | [nozhik.exe](https://github.com/desfpc/Nozhik/raw/refs/heads/master/binaries/windows-x64/nozhik.exe) | [build.json](windows-x64/build.json) |

Ссылки скачивания относятся к опубликованной ветке `master`; локальные изменения
станут доступны по ним только после push. Бинарники включены непосредственно в Git.
macOS Intel, Linux, установщики, подпись и notarization пока не поставляются.

macOS: скачайте `nozhik`, затем выполните в каталоге загрузки:

```sh
chmod +x nozhik
./nozhik /path/to/project
```

Windows: запустите `nozhik.exe` или `nozhik.exe C:\Projects\demo` в терминале.
Если каталог содержит DLL, скачайте их рядом с `.exe` (либо скачайте весь каталог).
Программа использует системные графические API; внешние PHP LSP-серверы
устанавливаются отдельно по инструкции в корневом README. Сборки не подписаны.

`build.json` хранит SHA-256 исходников, SHA-256 каждого файла, Rust target,
версию rustc и build host. `runtime_tested: false` означает, что успешная компиляция
и проверка формата файла не подтверждают ручную проверку GUI на целевой ОС.

## Обновление после правок

Из корня репозитория (Python 3.9+):

```sh
python3 scripts/update-binaries.py --target aarch64-apple-darwin
python3 scripts/update-binaries.py --target x86_64-pc-windows-gnu
python3 scripts/update-binaries.py --check
```

macOS → Windows требует `rustup target add x86_64-pc-windows-gnu` и
`brew install mingw-w64`. На Windows используйте
`python scripts/update-binaries.py --target x86_64-pc-windows-msvc` с MSVC Build Tools
и Perl. MSVC-сборка включает CRT статически; GNU-сборка копирует требуемые MinGW DLL.
Для macOS нужен Apple SDK/Xcode Command Line Tools. Без --target скрипт собирает
текущую поддерживаемую платформу. --check ничего не компилирует и завершается
ошибкой при несовпадении исходников или контрольных сумм.

После изменения исходников обновляйте обе платформы и добавляйте бинарники вместе
с исходниками в коммит. Хеш охватывает build inputs, включая незакоммиченные новые
файлы; docs-only изменения пересборки не требуют. Fingerprint не содержит Git SHA,
поэтому добавление самих бинарников в коммит не делает их устаревшими.
`.gitattributes` фиксирует LF, чтобы проверки совпадали на Windows/macOS/Linux.

Workflow [binaries.yml](../.github/workflows/binaries.yml) запускается после push
build inputs и вручную через Actions. Он собирает на macOS/Windows runners,
проверяет обе платформы и сохраняет их в текущую ветку отдельным bot-коммитом.
Готовые файлы также доступны как workflow artifacts. Требуется разрешённая запись
`GITHUB_TOKEN` в репозиторий; branch protection может потребовать отдельного
разрешения владельца. Более старый run не перезаписывает новую версию ветки.
Workflow не вызывается от правок одних бинарников и не создаёт цикл коммитов.
