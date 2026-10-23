use crate::{
    HostSnapshot, PluginManager, PluginStatus,
    keymap::{KeyBinding, KeyChord, KeymapConfig, KeymapProfile},
    manifest::PluginManifest,
    registry::{Action, ActionContext, When},
};
use nozhik_event_bus::{Event, EventBus};
use serde_json::{Value, json};
use std::{
    error::Error,
    path::Path,
    time::{Duration, Instant},
};
type TestResult = std::result::Result<(), Box<dyn Error>>;

fn manifest(id: &str) -> Value {
    json!({"id":id,"name":"PHP","version":"0.1.0","min_ide_version":"0.1.0","description":"PHP support","author":"Nozhik Team",
    "activation_events":["onLanguage:php","onFileExtension:.php"],
    "contributes":{"languages":[{"id":"php","extensions":[".php",".phtml"],"aliases":["PHP"]}],"commands":[{"id":"php.runScript","title":"PHP: Run Current Script","when":"editorOpen"}],
    "tabs":[{"id":"php.info","title":"PHP","description":"PHP tools"}],
    "configuration":{"php.executablePath":{"type":"string","default":"php","description":"Path to PHP binary"}}}})
}
fn install(root: &Path, id: &str, manifest: &Value) -> TestResult {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("plugin.json"), serde_json::to_vec(manifest)?)?;
    Ok(())
}
fn wait(host: &PluginManager, predicate: impl Fn(&HostSnapshot) -> bool) -> TestResult {
    let start = Instant::now();
    while !predicate(&host.snapshot()) {
        if start.elapsed() > Duration::from_secs(10) {
            return Err(format!("host timeout: {:?}", host.snapshot()).into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
#[test]
fn strict_manifest_and_semver_compatibility() -> TestResult {
    let source = manifest("org.nozhik.php");
    let parsed = PluginManifest::parse(&source.to_string())?;
    assert!(parsed.compatible_with(&"0.1.0".parse()?));
    assert!(parsed.compatible_with(&"0.2.0".parse()?));
    assert!(!parsed.compatible_with(&"0.1.0-alpha.1".parse()?));
    assert!(!parsed.compatible_with(&"0.0.9".parse()?));
    assert_eq!(
        PluginManifest::parse(&serde_json::to_string(&parsed)?)?.id,
        parsed.id
    );
    for (pointer, value) in [
        ("/unknown", json!(true)),
        ("/version", json!("nope")),
        ("/id", json!("../bad")),
        ("/min_ide_version", json!("^0.1")),
        ("/executable", json!("../bad")),
        ("/activation_events", json!(["onTypo:php"])),
        (
            "/contributes/commands/0/when",
            json!("editorOpen || selectedFile"),
        ),
        ("/contributes/languages/0/extensions", json!(["php"])),
        (
            "/contributes/configuration/php.executablePath/default",
            json!(1),
        ),
    ] {
        let mut invalid = source.clone();
        if pointer == "/unknown" || pointer == "/executable" {
            invalid[pointer.trim_start_matches('/')] = value;
        } else {
            *invalid.pointer_mut(pointer).ok_or("bad test pointer")? = value;
        }
        assert!(
            PluginManifest::parse(&invalid.to_string()).is_err(),
            "{pointer}"
        );
    }
    let mut duplicate = source.clone();
    let command = duplicate["contributes"]["commands"][0].clone();
    duplicate["contributes"]["commands"]
        .as_array_mut()
        .ok_or("commands")?
        .push(command);
    assert!(PluginManifest::parse(&duplicate.to_string()).is_err());
    let mut unknown = source;
    unknown["contributes"]["languages"][0]["typo"] = json!(true);
    assert!(PluginManifest::parse(&unknown.to_string()).is_err());
    Ok(())
}
#[test]
fn action_keypress_event_cycle_conflicts_and_context() -> TestResult {
    let bus = EventBus::new(16)?;
    let host = PluginManager::with_paths(bus.clone(), Vec::new(), None)?;
    let mut events = bus.subscribe();
    let actions = host.actions();
    actions.register(Action {
        id: "test.run".into(),
        title: "Run".into(),
        when: When("editorOpen && !selectedFile".into()),
        owner: None,
    })?;
    assert!(
        actions
            .register(Action {
                id: "test.run".into(),
                title: "Duplicate".into(),
                when: When::default(),
                owner: None
            })
            .is_err()
    );
    assert!(
        actions
            .register_batch(vec![
                Action {
                    id: "fresh.command".into(),
                    title: "Fresh".into(),
                    when: When::default(),
                    owner: None
                },
                Action {
                    id: "test.run".into(),
                    title: "Duplicate".into(),
                    when: When::default(),
                    owner: None
                },
            ])
            .is_err()
    );
    assert!(
        actions
            .available(ActionContext::default())?
            .iter()
            .all(|a| a.id != "fresh.command")
    );
    let chord = KeyChord {
        key: "R".into(),
        primary: true,
        ..KeyChord::default()
    };
    let keymap = host.keymap();
    let binding = |action: &str| KeyBinding {
        chord: chord.clone(),
        action: action.into(),
        when: When::default(),
    };
    let conflicts = keymap.replace(KeymapConfig {
        profile: KeymapProfile::Custom,
        bindings: vec![binding("old.action"), binding("test.run")],
    })?;
    assert_eq!(conflicts, vec![chord.label()]);
    assert!(
        keymap
            .dispatch(&chord, ActionContext::default(), &actions)
            .is_err()
    );
    assert!(events.try_recv().is_err());
    assert!(keymap.dispatch(
        &chord,
        ActionContext {
            editor_open: true,
            ..ActionContext::default()
        },
        &actions
    )?);
    assert_eq!(
        *events.try_recv()?,
        Event::ActionTriggered("test.run".into())
    );
    let mut invalid = binding("test.run");
    invalid.when = When("badKey".into());
    assert!(
        keymap
            .replace(KeymapConfig {
                profile: KeymapProfile::Custom,
                bindings: vec![invalid]
            })
            .is_err()
    );
    assert_eq!(keymap.bindings()?.len(), 1);

    Ok(())
}
#[test]
fn discovery_activation_disable_and_project_replacement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let global = dir.path().join("global");
    install(&global, "php", &manifest("org.nozhik.php"))?;
    std::fs::create_dir_all(global.join("bad"))?;
    std::fs::write(global.join("bad/plugin.json"), "{invalid")?;
    let bus = EventBus::new(32)?;
    let host = PluginManager::with_paths(bus.clone(), vec![global], None)?;
    wait(&host, |s| s.plugins.len() == 1)?;
    assert_eq!(host.snapshot().plugins[0].status, PluginStatus::Enabled);
    assert!(
        host.snapshot()
            .diagnostics
            .iter()
            .any(|d| d.contains("bad"))
    );
    assert_eq!(
        host.snapshot().extensions.get(".phtml").map(String::as_str),
        Some("php")
    );
    assert!(
        host.snapshot()
            .configuration
            .contains_key("php.executablePath")
    );
    assert!(host.snapshot().tabs.contains_key("php.info"));
    bus.publish(Event::FileOpened(dir.path().join("index.PHP")));
    wait(&host, |s| s.plugins[0].status == PluginStatus::Active)?;
    host.set_enabled("org.nozhik.php", false)?;
    wait(&host, |s| s.plugins[0].status == PluginStatus::Disabled)?;
    assert!(host.snapshot().extensions.is_empty());
    assert!(
        host.actions()
            .available(ActionContext {
                editor_open: true,
                ..ActionContext::default()
            })?
            .is_empty()
    );
    host.set_enabled("org.nozhik.php", true)?;
    wait(&host, |s| s.plugins[0].status == PluginStatus::Active)?;
    let project = dir.path().join("project");
    let mut local = manifest("org.nozhik.local");
    local["contributes"] = json!({});
    install(&project.join(".nozhik/plugins"), "local", &local)?;
    host.scan(Some(project))?;
    wait(&host, |s| s.plugins.len() == 2)?;
    host.scan(None)?;
    wait(&host, |s| s.plugins.len() == 1)?;
    Ok(())
}
#[test]
fn dependency_versions_cycles_collisions_and_cascade() -> TestResult {
    let dir = tempfile::tempdir()?;
    let base = manifest("org.nozhik.base");
    install(dir.path(), "base", &base)?;
    for (id, dependencies) in [
        ("org.nozhik.dependent", json!({"org.nozhik.base":"^0.1"})),
        ("org.nozhik.missing", json!({"org.nozhik.absent":"*"})),
        ("org.nozhik.version", json!({"org.nozhik.base":">=1.0"})),
        ("org.nozhik.cycle1", json!({"org.nozhik.cycle2":"*"})),
        ("org.nozhik.cycle2", json!({"org.nozhik.cycle1":"*"})),
    ] {
        let mut value = manifest(id);
        value["contributes"] = json!({});
        value["dependencies"] = dependencies;
        install(dir.path(), id, &value)?;
    }
    install(dir.path(), "collision", &manifest("org.nozhik.collision"))?;
    let host = PluginManager::with_paths(EventBus::new(64)?, vec![dir.path().into()], None)?;
    wait(&host, |s| s.plugins.len() == 7)?;
    let snapshot = host.snapshot();
    assert_eq!(
        snapshot
            .plugins
            .iter()
            .filter(|p| matches!(p.status, PluginStatus::Failed(_)))
            .count(),
        5
    );
    host.set_enabled("org.nozhik.base", false)?;
    wait(&host, |s| {
        s.plugins.iter().filter(|p| p.enabled()).count() == 0
    })?;
    host.set_enabled("org.nozhik.dependent", true)?;
    wait(&host, |s| {
        s.plugins.iter().filter(|p| p.enabled()).count() == 2
    })?;
    Ok(())
}
#[test]
fn keymap_file_reload_keeps_previous_on_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("keymap.json");
    std::fs::write(&path, r#"{"profile":"VSCode"}"#)?;
    let host = PluginManager::with_paths(EventBus::new(16)?, Vec::new(), Some(path.clone()))?;
    wait(&host, |_| {
        host.keymap()
            .bindings()
            .is_ok_and(|b| b.iter().any(|b| b.chord.key == "P"))
    })?;
    std::fs::write(&path, "{bad")?;
    wait(&host, |s| {
        s.diagnostics.iter().any(|s| s.contains("reload rejected"))
    })?;
    assert!(host.keymap().bindings()?.iter().any(|b| b.chord.key == "P"));
    std::fs::write(&path, "")?;
    wait(&host, |s| {
        s.diagnostics
            .iter()
            .filter(|s| s.contains("reload rejected"))
            .count()
            >= 2
    })?;
    assert!(host.keymap().bindings()?.iter().any(|b| b.chord.key == "P"));
    std::fs::write(&path, r#"{"profile":"Custom","bindings":[]}"#)?;
    wait(&host, |_| {
        host.keymap().bindings().is_ok_and(|b| b.is_empty())
    })?;
    Ok(())
}
#[test]
fn real_process_rpc_crash_timeout_and_oversize_isolation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let executable = format!("fixture{}", std::env::consts::EXE_SUFFIX);
    let binary = dir.path().join(&executable);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(rustc)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_plugin.rs"))
        .arg("-o")
        .arg(&binary)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = dir.path().join("plugins");
    for mode in ["ok", "crash", "hang", "oversize"] {
        let mut value = manifest(&format!("org.nozhik.{mode}"));
        value["contributes"] = json!({"commands":[{"id":format!("{mode}.run"),"title":mode}]});
        value["activation_events"] = json!([format!("onCommand:{mode}.run")]);
        value["executable"] = json!(executable);
        value["args"] = json!([mode]);
        install(&root, mode, &value)?;
        std::fs::copy(&binary, root.join(mode).join(&executable))?;
    }
    let host = PluginManager::with_paths(EventBus::new(32)?, vec![root.clone()], None)?;
    wait(&host, |s| s.plugins.len() == 4)?;
    let actions = host.actions();
    for mode in ["ok", "crash", "hang", "oversize"] {
        actions.execute(&format!("{mode}.run"), ActionContext::default())?;
        wait(&host, |s| {
            s.plugins.iter().any(|p| {
                p.manifest.id == format!("org.nozhik.{mode}")
                    && if mode == "ok" {
                        p.status == PluginStatus::Active
                    } else {
                        matches!(p.status, PluginStatus::Failed(_))
                    }
            })
        })?;
    }
    assert!(std::fs::read_to_string(root.join("ok/executed"))?.contains("ok.run"));
    assert_eq!(actions.available(ActionContext::default())?.len(), 1);
    host.set_enabled("org.nozhik.ok", false)?;
    wait(&host, |s| s.plugins.iter().all(|p| !p.enabled()))?;
    drop(host);
    drop(actions);
    // A pending initialize must not retain EventBus senders for the full RPC timeout.
    std::fs::remove_file(root.join("hang/started"))?;
    let bus = EventBus::new(16)?;
    let mut closed = bus.subscribe();
    let host = PluginManager::with_paths(bus.clone(), vec![root.clone()], None)?;
    wait(&host, |s| s.plugins.len() == 4)?;
    host.actions()
        .execute("hang.run", ActionContext::default())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.join("hang/started").exists() {
        if Instant::now() > deadline {
            return Err("hanging process did not start".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(host);
    drop(bus);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match closed.try_recv() {
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            _ if Instant::now() > deadline => {
                return Err("host did not cancel shutdown promptly".into());
            }
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    Ok(())
}

#[test]
fn platform_equivalent_chords_conflict() -> TestResult {
    let keymap = crate::keymap::KeymapRegistry::default();
    let chord = KeyChord {
        key: "R".into(),
        primary: true,
        ..KeyChord::default()
    };
    let binding = |action: &str| KeyBinding {
        chord: chord.clone(),
        action: action.into(),
        when: When::default(),
    };
    let mut equivalent = chord.clone();
    equivalent.primary = false;
    if cfg!(target_os = "macos") {
        equivalent.cmd = true;
    } else {
        equivalent.ctrl = true;
    }
    let conflicts = keymap.replace(KeymapConfig {
        profile: KeymapProfile::Custom,
        bindings: vec![
            binding("old.action"),
            KeyBinding {
                chord: equivalent,
                action: "test.run".into(),
                when: When::default(),
            },
        ],
    })?;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(keymap.bindings()?.len(), 1);

    Ok(())
}
