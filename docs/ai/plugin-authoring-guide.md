# Разработка расширений Nozhik — контракт Phase 4

Расширение распространяется отдельно от IDE. Nozhik читает `plugin.json`,
регистрирует декларативные возможности и при активации запускает исполняемый
файл отдельным процессом. Поддерживаемая версия протокола — JSON-RPC 2.0
поверх stdio, один JSON-объект на строку UTF-8 (NDJSON).

## Структура установки

```text
~/.nozhik/plugins/
  org.nozhik.php/
    plugin.json
    bin/
      php-plugin       # php-plugin.exe на Windows
<project>/.nozhik/plugins/
  org.example.tools/
    plugin.json
    bin/tools-plugin
```

Глобальный каталог сканируется первым, локальный — вторым; внутри каталога
порядок лексикографический. При одинаковом ID первая установка побеждает,
последующие дают диагностику. Отсутствующий каталог допустим; повреждённый
манифест не мешает обнаружить остальные плагины. Установка — копирование папки
и **Settings → Plugins… → Rescan installed plugins**. Удаление — удаление папки
и повторное сканирование. При смене проекта старые процессы останавливаются,
локальные вклады заменяются; глобальные плагины обнаруживаются заново.

В этой версии валидные плагины включаются при сканировании. Enable/Disable
работает без перезапуска; выбор не сохраняется между сканированиями/запусками.
Отключение зависимости также отключает её транзитивных потребителей. Enable
потребителя включает необходимые зависимости. Зависимости должны быть установлены
заранее: загрузки пакетов из сети нет.

## Строгий манифест

```json
{
  "id": "org.nozhik.php",
  "name": "PHP Language Support",
  "version": "0.1.0",
  "min_ide_version": "0.1.0",
  "description": "PHP syntax and language server integration",
  "author": "Nozhik Team",
  "activation_events": [
    "onLanguage:php",
    "onFileExtension:.php",
    "onFileExtension:.phtml",
    "onCommand:php.runScript"
  ],
  "executable": "bin/php-plugin",
  "args": [],
  "dependencies": {},
  "contributes": {
    "languages": [
      { "id": "php", "extensions": [".php", ".phtml"], "aliases": ["PHP", "php"] }
    ],
    "commands": [
      { "id": "php.runScript", "title": "PHP: Run Current Script", "when": "editorOpen" }
    ],
    "configuration": {
      "php.executablePath": {
        "type": "string",
        "default": "php",
        "description": "Path to PHP binary"
      }
    },
    "tabs": [
      { "id": "php.tools", "title": "PHP Tools", "description": "PHP plugin information" }
    ]
  }
}
```

Обязательны `id`, `name`, `version`, `min_ide_version`, `description`, `author`
и `activation_events`. `contributes`, его коллекции, `dependencies`, `args`
имеют пустые defaults; `executable` необязателен. Без executable расширение
декларативное: доступны ассоциации, настройки и вкладки; команда публикует
уведомление и диагностику об отсутствии исполняемого обработчика.

Неизвестные поля запрещены на всех структурных уровнях. Используйте
`PluginManifest::parse`, который выполняет и serde-десериализацию, и семантическую
валидацию; прямой `serde_json::from_str` проверяет только структуру.
ID состоят из непустых сегментов ASCII букв/цифр/`_`/`-`, разделённых точками;
ID плагина должен содержать точку. Пространство команд `ui.*` зарезервировано IDE.
Имена и автор не могут быть пустыми. Список языковых расширений непустой,
суффиксы начинаются с точки, сравнение не учитывает регистр.

`version` и `min_ide_version` — полные SemVer версии. Минимум включителен;
`0.1.0-alpha.1 < 0.1.0`. Значения `dependencies` — semver requirements,
например `{"org.example.runtime": "^0.1.0"}`. Отсутствующая/несовместимая
зависимость и цикл дают Failed. Конфликты команд, языков, расширений, вкладок
и ключей конфигурации отклоняют регистрацию соответствующего плагина.
`configuration.type` поддерживает string, boolean, integer, number, array, object;
`default` должен соответствовать указанному типу.

Executable должен находиться внутри папки плагина, включая результат разрешения
symlink; абсолютные пути, `..` и Windows drive prefixes запрещены. Аргументы
передаются напрямую, без shell. Для каждой ОС поставляйте соответствующий бинарник
и путь в её варианте манифеста. На Unix требуется executable permission.

## Активация и процессный протокол

Поддерживаются `*` (сразу после включения), `onLanguage:<id>`,
`onFileExtension:<.ext>` и `onCommand:<id>`. Успешное открытие файла публикует
`FileOpened`: host сопоставляет суффиксы с включёнными языками и активирует
подходящие плагины. Повторное открытие также публикует событие. Выполнение любой
зарегистрированной команды её плагина автоматически активирует владельца;
явный `onCommand` допустим для описания этого намерения. Зависимости
инициализируются раньше потребителя.

Первый запрос (порядок JSON-полей не является частью протокола):

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"ide_version":"0.1.0","plugin_id":"org.nozhik.php","configuration":{"php.executablePath":{"type":"string","default":"php","description":"Path to PHP binary"}}}}
```

Ответ, завершающийся LF:

```json
{"jsonrpc":"2.0","id":1,"result":{}}
```

Затем host посылает команды, увеличивая `id`:

```json
{"jsonrpc":"2.0","id":2,"method":"executeCommand","params":{"command":"php.runScript"}}
```

Плагин отвечает соответствующим `id` и `result`, например `null`. JSON-RPC
`error`, несовпадающий ID, неверный JSON, EOF и завершение процесса переводят
плагин в Failed, завершают его процесс, удаляют вклады и отключают потребителей.
На каждый запрос отведено 3 секунды, ответ ограничен 64 KiB вместе с LF.
Stdout предназначен только для ответов; unsolicited notifications и вызовы host API
пока не поддерживаются. Stderr дочернего процесса направлен в null.
Значение result подтверждает выполнение, но пока не визуализируется IDE.

Запросы идут через ограниченную mpsc-очередь на отдельный host worker; при
переполнении API возвращает ошибку. В текущей версии worker последовательно
обслуживает плагины, поэтому зависший вызов задерживает следующие операции
до таймаута, сохраняя отзывчивость UI. Disable завершает и reap-ит прямой
дочерний процесс; закрытие IDE отменяет ожидающий RPC и завершает процессы.
Плагин отвечает за собственные дочерние процессы.

Изоляция процесса защищает IDE от panic/crash расширения. Это не OS sandbox:
плагин работает с правами текущего пользователя. LSP, linting, редактирование
буфера через RPC, Tree-sitter, произвольные UI-компоненты и БД API относятся
к следующим итерациям. Сейчас `languages` задаёт ассоциации, `configuration` —
валидируемые defaults в snapshot и initialize, `tabs` — типизированные
метаданные для generic-панели **Plugins → Extension tabs**.

## API действий и контексты

`PluginManager::actions()` возвращает общий `ActionRegistry`, который дешёво
клонируется между потоками. Пример регистрации встроенного bus-действия:

```rust
use nozhik_plugin_host::registry::{Action, ActionContext, When};
fn example(manager: &nozhik_plugin_host::PluginManager) -> nozhik_plugin_host::Result<()> {
let actions = manager.actions();
actions.register(Action {
    id: "example.inspect".into(),
    title: "Inspect Selected File".into(),
    when: When("projectOpen && selectedFile".into()),
    owner: None,
})?;
actions.execute("example.inspect", ActionContext {
    project_open: true,
    selected_file: true,
    ..ActionContext::default()
})?;
Ok(())
}
```

`owner: None` публикует `Event::ActionTriggered(id)`. `owner: Some(plugin_id)`
используется host для команд расширений: сначала надёжная локальная очередь,
затем bus-уведомление. Шина остаётся best-effort, уведомление не подтверждает
успех RPC. Обработчики IDE выполняются потребителем шины; callbacks расширений
на UI-потоке отсутствуют. `available(context)` отдаёт снимок без удержания
lock; `unregister_owner(id)` удаляет все команды владельца.

`when` — пустая строка либо конъюнкция через `&&` с необязательным `!` перед
каждым ключом. Доступны `editorOpen`, `editorFocus`, `selectedFile`, `projectOpen`.
Скобки, OR, сравнения и неизвестные ключи отклоняются. Проверка применяется
и к Actions в поиске, и при фактической диспетчеризации.

## Хоткеи и горячая смена профиля

Создайте `~/.nozhik/keymap.json`:

```json
{
  "profile": "VSCode",
  "bindings": [
    {
      "chord": { "key": "R", "primary": true, "alt": true, "shift": false },
      "action": "php.runScript",
      "when": "editorOpen"
    }
  ]
}
```

Профили: `JetBrains` (default), `VSCode`, `Custom`. Custom содержит только явно
заданные bindings. JetBrains: Ctrl/Cmd+Shift+A, Ctrl/Cmd+Shift+O, Ctrl/Cmd+E
открывают поиск; VSCode: Ctrl/Cmd+Shift+P. Оба включают Ctrl/Cmd+S и
Ctrl/Cmd+Shift+G. Double Shift остаётся отдельным жестом открытия поиска.
`primary` означает Ctrl на Windows/Linux и Cmd на macOS; `ctrl`, `cmd`, `alt`,
`shift` задают отдельные модификаторы. `primary` нельзя смешивать с `ctrl`/`cmd`.
Клавиши: A–Z, 0–9, F1–F35, Enter, Escape, Space, Tab, Backspace, Delete, Insert,
Home, End, PageUp/PageDown, ArrowUp/Down/Left/Right. Сочетания редактора
undo/redo/навигации и Git commit пока обрабатываются их собственными views.

Bindings накладываются по порядку: последний эквивалентный chord побеждает
(с учётом текущей платформы), конфликт фиксируется в диагностике. Неизвестный
action ID допустим до установки расширения; UI применяет binding только при
доступном действии. `when` binding и действия должны выполняться одновременно.

Worker раз в секунду перечитывает файл и атомарно заменяет весь профиль.
Отсутствующий файл возвращает JetBrains; пустой/невалидный файл сохраняет
предыдущий профиль и даёт диагностику. IDE не перезаписывает этот файл.
Search Everywhere → Actions показывает текущие shortcuts; Enter выполняет
выбранную команду. `KeymapRegistry::replace` и `dispatch` доступны и без UI.

Исходные API: [Tokio process Command](https://docs.rs/tokio/latest/tokio/process/struct.Command.html),
[semver VersionReq](https://docs.rs/semver/latest/semver/struct.VersionReq.html).
