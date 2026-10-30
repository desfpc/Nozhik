//! SSH profile forms, lazy remote tree and editor upload integration.
use crate::{
    NozhikUi,
    remote_worker::{Command, Intent, Reply, Worker},
};
use nozhik_remote_ssh::{
    AuthMethod, PortForward, SshProfile,
    credentials::{Secret, same_identity},
    sftp::RemoteFileEntry,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use zeroize::Zeroize;

#[derive(Default)]
pub struct RemoteUi {
    worker: Option<Worker>,
    profiles: Vec<SshProfile>,
    profiles_loaded: bool,
    connected: HashSet<String>,
    tree: HashMap<(String, String), Vec<RemoteFileEntry>>,
    expanded: HashSet<(String, String)>,
    pending: HashSet<(String, String)>,
    browser: Option<String>,
    pub documents: HashMap<PathBuf, (String, String)>,
    form: Option<Form>,
    login: Option<(SshProfile, Intent)>,
    trust: Option<(SshProfile, String, Intent)>,
    secret: String,
    login_focus: bool,
    host_key_error: Option<String>,
    login_error: String,
    delete: Option<String>,
    invalidated: HashSet<PathBuf>,
    profile_busy: bool,
    profile_error: String,
}
struct Form {
    profile: SshProfile,
    key_path: String,
    auth: u8,
    original: Option<String>,
    error: String,
}
impl Default for Form {
    fn default() -> Self {
        Self {
            profile: SshProfile {
                name: String::new(),
                host: String::new(),
                port: 22,
                username: String::new(),
                auth: AuthMethod::Password,
                host_key_sha256: String::new(),
                save_password: false,
                forwards: Vec::new(),
            },
            key_path: "~/.ssh/id_ed25519".into(),
            auth: 0,
            original: None,
            error: String::new(),
        }
    }
}
impl Form {
    fn edit(profile: SshProfile) -> Self {
        let (auth, key_path) = match &profile.auth {
            AuthMethod::Password => (0, "~/.ssh/id_ed25519".into()),
            AuthMethod::PrivateKeyFile { path } => (1, path.to_string_lossy().into_owned()),
            AuthMethod::Agent => (2, "~/.ssh/id_ed25519".into()),
        };
        Self {
            original: Some(profile.name.clone()),
            profile,
            auth,
            key_path,
            error: String::new(),
        }
    }
}
impl RemoteUi {
    pub fn database_profiles(&self) -> &[SshProfile] {
        &self.profiles
    }
    pub fn connect_database(&self, name: &str, id: u64) -> bool {
        self.profiles
            .iter()
            .find(|p| p.name == name)
            .is_some_and(|p| self.connect(p.clone(), Intent::Database(id)))
    }
    pub const fn modal_open(&self) -> bool {
        self.form.is_some()
            || self.login.is_some()
            || self.trust.is_some()
            || self.host_key_error.is_some()
            || self.delete.is_some()
    }
    pub fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
    fn queue(&self, command: Command) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send(command))
    }
    pub fn document_invalidated(&self, path: &std::path::Path) -> bool {
        self.invalidated.contains(path)
    }
    pub fn save(&self, path: &std::path::Path, revision: u64, text: ropey::Rope) -> bool {
        if self.invalidated.contains(path) {
            return false;
        }
        if let Some((name, remote)) = self.documents.get(path) {
            return self.queue(Command::Write(
                name.clone(),
                remote.clone(),
                path.into(),
                revision,
                text,
            ));
        }
        false
    }
    fn connect(&self, profile: SshProfile, intent: Intent) -> bool {
        if self.connected.contains(&profile.name) {
            self.queue(Command::Connect(profile, None, intent))
        } else {
            self.queue(Command::Verify(profile, intent))
        }
    }
    fn cancel_trust(&mut self) {
        self.trust = None;
        self.secret.zeroize();
    }
    fn verified(&mut self, profile: SshProfile, intent: Intent) -> bool {
        if matches!(profile.auth, AuthMethod::Agent) || profile.save_password {
            self.queue(Command::Connect(profile, None, intent))
        } else {
            self.require_credentials(profile, intent, String::new());
            true
        }
    }
    fn require_credentials(&mut self, profile: SshProfile, intent: Intent, error: String) {
        self.login = Some((profile, intent));
        self.login_error = error;
        self.login_focus = true;
        self.secret.zeroize();
    }
    fn profiles_updated(&mut self, profiles: Vec<SshProfile>) {
        for new in &profiles {
            if !self.profiles.iter().any(|old| old.name == new.name) {
                self.connected.remove(&new.name);
            }
        }
        for old in &self.profiles {
            let new = profiles.iter().find(|new| same_identity(old, new));
            if new.is_none_or(|new| {
                new.host_key_sha256 != old.host_key_sha256 || new.forwards != old.forwards
            }) {
                self.connected.remove(&old.name);
            }
            if new.is_none_or(|new| new.host_key_sha256 != old.host_key_sha256) {
                self.invalidated.extend(
                    self.documents
                        .iter()
                        .filter(|(_, (host, _))| host == &old.name)
                        .map(|(path, _)| path.clone()),
                );
                self.connected.remove(&old.name);
                self.tree.retain(|(host, _), _| host != &old.name);
                self.pending.retain(|(host, _)| host != &old.name);
                if self.browser.as_ref() == Some(&old.name) {
                    self.browser = None;
                }
            }
        }
        self.profiles = profiles;
        self.profiles_loaded = true;
    }
}
impl NozhikUi {
    pub(crate) fn receive_remote(&mut self, ctx: &egui::Context) {
        if self.remote.worker.is_none() {
            let ctx = ctx.clone();
            self.remote.worker = Some(Worker::start(
                self.settings.path().with_file_name("ssh_hosts.json"),
                Arc::new(move || ctx.request_repaint()),
            ));
        }
        let replies: Vec<_> = self
            .remote
            .worker
            .as_mut()
            .into_iter()
            .flat_map(|w| std::iter::from_fn(|| w.output.try_recv().ok()).take(128))
            .collect();
        for reply in replies {
            self.accept_remote_reply(reply, ctx);
        }
        if !self.workspace.background {
            self.remote_dialogs(ctx);
        }
    }
    fn accept_remote_reply(&mut self, reply: Reply, ctx: &egui::Context) {
        match reply {
            Reply::HostKeyChanged(message) => self.remote.host_key_error = Some(message),
            Reply::TrustRequired(profile, pin, intent) => {
                self.remote.trust = Some((profile, pin, intent));
            }
            Reply::Verified(profile, intent) => {
                if !self.remote.verified(profile, intent) {
                    self.status = "SSH queue full/stopped".into();
                }
            }
            Reply::Profiles(profiles) => self.remote.profiles_updated(profiles),
            Reply::ProfilesChanged(profiles) => {
                self.remote.profiles_updated(profiles);
                self.remote.form = None;
                self.remote.delete = None;
                self.remote.profile_busy = false;
                self.remote.profile_error.clear();
                self.status = "SSH profiles updated".into();
            }
            Reply::CredentialsRequired(profile, intent, error) => {
                self.remote.require_credentials(profile, intent, error);
            }
            Reply::DatabaseFailed(id, error) => self.database.cancel_ssh(id, error),
            Reply::DatabaseSession(id, session) => self.database.accept_ssh(id, session),
            Reply::Connected(name) => {
                self.remote.connected.insert(name.clone());
                self.status = format!("SSH {name}: Connected");
            }
            Reply::Disconnected(name) => {
                self.remote.connected.remove(&name);
                self.remote.tree.retain(|(host, _), _| host != &name);
                self.remote.pending.retain(|(host, _)| host != &name);
                self.status = format!("SSH {name}: Offline");
            }
            Reply::Shell(name, shell) => {
                self.remote.connected.insert(name.clone());
                let panel = self
                    .terminal
                    .get_or_insert_with(|| crate::terminal_panel::TerminalPanel::new(ctx));
                match panel.create_remote(&name, Box::new(shell)) {
                    Ok(()) => self.bottom = Some(crate::terminal_panel::TERMINAL_LABEL),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Reply::Listed(name, path, entries) => {
                self.remote.connected.insert(name.clone());
                self.remote.browser = Some(name.clone());
                self.remote.pending.remove(&(name.clone(), path.clone()));
                self.remote.tree.insert((name, path), entries);
            }
            Reply::Read(name, remote, cache, text) => {
                self.loaded_file(cache.clone(), &text, false);
                self.remote
                    .documents
                    .insert(cache.clone(), (name.clone(), remote.clone()));
                if let Some(document) = self.documents.iter_mut().find(|d| d.path == cache) {
                    document.name = format!(
                        "[SSH: {name}] {}",
                        remote.rsplit('/').next().unwrap_or(&remote)
                    );
                }
            }
            Reply::Saved(path, revision) => {
                self.saved(&path, revision);
                if let Some(document) = self.documents.iter_mut().find(|d| d.path == path)
                    && document.buffer.snapshot().revision == revision
                {
                    document.buffer.mark_saved();
                }
                self.status = "Saved via SFTP".into();
            }
            Reply::Message(message) => self.status = message,
            Reply::Error(error) => {
                if !self.integration.saving.is_empty() {
                    self.integration.save_error = Some(error.clone());
                    self.integration.saving.clear();
                }
                self.remote.profile_busy = false;
                self.remote.profile_error.clone_from(&error);
                self.remote.pending.clear();
                if let Some(form) = &mut self.remote.form {
                    form.error.clone_from(&error);
                }
                self.status = format!("SSH: {error}");
            }
        }
    }
    pub(crate) fn remote_panel(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.remote.profiles_loaded,
                egui::Button::new("+ Add SSH host"),
            )
            .clicked()
        {
            self.remote.form = Some(Form::default());
        }
        let profiles = self.remote.profiles.clone();
        for profile in profiles {
            ui.separator();
            ui.label(format!(
                "{} · {}",
                profile.name,
                if self.remote.connected.contains(&profile.name) {
                    "Connected"
                } else {
                    "Offline"
                }
            ));
            ui.weak(format!(
                "{}@{}:{}",
                profile.username, profile.host, profile.port
            ));
            ui.horizontal(|ui| {
                if ui.button("Edit").clicked() {
                    self.remote.form = Some(Form::edit(profile.clone()));
                }
                if ui.button("Delete…").clicked() {
                    self.remote.delete = Some(profile.name.clone());
                    self.remote.profile_error.clear();
                }
                if profile.save_password
                    && ui.button("Forget password").clicked()
                    && !self
                        .remote
                        .queue(Command::ForgetPassword(profile.name.clone()))
                {
                    self.status = "SSH queue full/stopped".into();
                }
            });
            for (label, intent) in [
                ("Test Connection", Intent::Test),
                ("Open Terminal", Intent::Terminal),
                ("Browse SFTP", Intent::Browse),
                ("Start Tunnels", Intent::Tunnels),
            ] {
                if ui.button(label).clicked() && !self.remote.connect(profile.clone(), intent) {
                    self.status = "SSH queue full/stopped".into();
                }
            }
            if self.remote.connected.contains(&profile.name) {
                if ui.button("Disconnect").clicked() {
                    self.remote.queue(Command::Disconnect(profile.name.clone()));
                }
                if ui.button("Stop Tunnels").clicked() {
                    self.remote
                        .queue(Command::StopTunnels(profile.name.clone()));
                }
            }
        }
        if let Some(name) = self.remote.browser.clone() {
            ui.separator();
            ui.label(format!("SFTP: {name}"));
            if ui.button("Refresh").clicked() {
                self.remote.tree.clear();
                self.remote.pending.clear();
                self.remote
                    .queue(Command::List(name.clone(), ".".into(), true));
            }
            let mut loads = Vec::new();
            let mut open = None;
            egui::ScrollArea::both().show(ui, |ui| {
                tree_row(ui, &name, ".", &mut self.remote, &mut loads, &mut open);
            });
            for path in loads {
                let key = (name.clone(), path.clone());
                if !self.remote.pending.contains(&key)
                    && self.remote.queue(Command::List(name.clone(), path, false))
                {
                    self.remote.pending.insert(key);
                }
            }
            if let Some(path) = open {
                if let Some(cache) = self.remote.documents.iter().find_map(|(cache, identity)| {
                    (identity == &(name.clone(), path.clone())
                        && !self.remote.invalidated.contains(cache))
                    .then(|| cache.clone())
                }) {
                    self.open_file(cache);
                } else {
                    self.remote.queue(Command::Read(name, path));
                }
            }
        }
    }
    fn delete_host_dialog(&mut self, ctx: &egui::Context) {
        let mut remove = None;
        let mut cancel = false;
        if let Some(name) = &self.remote.delete {
            egui::Modal::new(egui::Id::new("delete_ssh_profile")).show(ctx, |ui| {
                ui.heading(format!("Delete SSH host {name}?"));
                ui.label("The profile and saved password will be removed. Active connections will close; open editor buffers keep their text.");
                if !self.remote.profile_error.is_empty() { ui.colored_label(crate::theme::ERROR, &self.remote.profile_error); }
                if ui.add_enabled(!self.remote.profile_busy, egui::Button::new("Delete host")).clicked() { remove = Some(name.clone()); }
                cancel = ui.add_enabled(!self.remote.profile_busy, egui::Button::new("Cancel")).clicked();
            });
        }
        if let Some(name) = remove {
            if self.remote.queue(Command::DeleteProfile(name)) {
                self.remote.profile_busy = true;
            } else {
                self.status = "SSH queue full/stopped".into();
            }
        }
        if cancel {
            self.remote.delete = None;
        }
    }
    fn host_trust_dialog(&mut self, ctx: &egui::Context) {
        let mut close = false;
        if let Some(error) = &self.remote.host_key_error {
            egui::Modal::new(egui::Id::new("ssh_changed_key")).show(ctx, |ui| {
                ui.heading("SSH host key changed — connection blocked");
                ui.label(error);
                ui.label(
                    "Contact the server administrator before changing the saved trust record.",
                );
                close = ui.button("Close").clicked();
            });
        }
        if close {
            self.remote.host_key_error = None;
        }
        let mut accept = false;
        let mut cancel = false;
        if let Some((profile, pin, _)) = &self.remote.trust {
            egui::Modal::new(egui::Id::new("ssh_host_trust")).show(ctx, |ui| {
                ui.heading("Trust SSH server?");
                ui.label(format!("First connection to {}:{}", profile.host, profile.port));
                ui.label("This server's identity has not been saved yet.");
                ui.label("SHA256 host key fingerprint:");
                ui.monospace(pin);
                ui.label("Compare with the server administrator if needed. Trusting saves this key for future connections.");
                ui.horizontal(|ui| {
                    accept = ui.button("Trust and Continue").clicked();
                    cancel = ui.add_enabled(!self.remote.profile_busy, egui::Button::new("Cancel")).clicked();
                });
            });
        }
        if accept
            && let Some((profile, pin, intent)) = self.remote.trust.take()
            && !self.remote.queue(Command::Trust(profile, pin, intent))
        {
            self.status = "SSH queue full/stopped".into();
        }
        if cancel {
            if let Some((_, _, Intent::Database(id))) = &self.remote.trust {
                self.database
                    .cancel_ssh(*id, "SSH connection cancelled".into());
            }
            self.remote.cancel_trust();
            self.status = "SSH connection cancelled; host was not trusted".into();
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Profile and credential forms share a single egui window lifecycle"
    )]
    fn remote_dialogs(&mut self, ctx: &egui::Context) {
        let mut save = None;
        let mut test = None;
        let mut cancel = false;
        if self.remote.login.is_none()
            && self.remote.trust.is_none()
            && self.remote.host_key_error.is_none()
            && let Some(form) = &mut self.remote.form
        {
            egui::Modal::new(egui::Id::new("ssh_profile")).show(ctx, |ui| {
                ui.heading(if form.original.is_some() { "Edit SSH host" } else { "Add SSH host" });
                if form.original.is_some() { ui.weak("Changing connection identity removes its saved password and disconnects active sessions."); }
                for (label, value) in [
                    ("Name", &mut form.profile.name),
                    ("Host", &mut form.profile.host),
                    ("User", &mut form.profile.username),
                ] {
                    ui.label(label);
                    ui.text_edit_singleline(value);
                }
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut form.profile.port).range(1..=65535));
                egui::ComboBox::from_label("Auth Type")
                    .selected_text(match form.auth {
                        1 => "Private Key",
                        2 => "SSH Agent",
                        _ => "Password",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut form.auth, 0, "Password");
                        ui.selectable_value(&mut form.auth, 1, "Private Key");
                        ui.selectable_value(&mut form.auth, 2, "SSH Agent");
                    });
                if form.auth == 0 {
                    ui.weak(
                        "Password will be requested when connecting, after server verification.",
                    );
                }
                if form.auth == 1 {
                    ui.label("Private key path");
                    ui.text_edit_singleline(&mut form.key_path);
                }
                ui.label("Local Port Forwards");
                let mut remove = None;
                for (index, forward) in form.profile.forwards.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut forward.local_port).prefix("Local "));
                        ui.text_edit_singleline(&mut forward.remote_host);
                        ui.add(egui::DragValue::new(&mut forward.remote_port).prefix("Remote ").range(1..=65535));
                        if ui.button("Remove").clicked() { remove = Some(index); }
                    });
                }
                if let Some(index) = remove { form.profile.forwards.remove(index); }
                if ui.button("+ Add port forward").clicked() { form.profile.forwards.push(PortForward { local_port: 0, remote_host: "127.0.0.1".into(), remote_port: 5432 }); }
                form.profile.auth = match form.auth {
                    1 => AuthMethod::PrivateKeyFile {
                        path: form.key_path.clone().into(),
                    },
                    2 => AuthMethod::Agent,
                    _ => AuthMethod::Password,
                };
                if !form.error.is_empty() {
                    ui.colored_label(crate::theme::ERROR, &form.error);
                }
                ui.horizontal(|ui| {
                    let valid = form.profile.validate();
                    if ui.add_enabled(!self.remote.profile_busy, egui::Button::new("Save profile")).clicked() {
                        match &valid {
                            Ok(()) => save = Some((form.original.clone(), form.profile.clone())),
                            Err(e) => form.error = e.to_string(),
                        }
                    }
                    if ui.add_enabled(!self.remote.profile_busy, egui::Button::new("Test Connection")).clicked() {
                        match valid {
                            Ok(()) => test = Some(form.profile.clone()),
                            Err(e) => form.error = e.to_string(),
                        }
                    }
                    cancel = ui.add_enabled(!self.remote.profile_busy, egui::Button::new("Cancel")).clicked();
                });
            });
        }
        if let Some((original, profile)) = save {
            if self.remote.queue(Command::UpsertProfile(original, profile)) {
                self.remote.profile_busy = true;
            } else {
                self.status = "SSH queue full/stopped".into();
            }
        }
        if let Some(profile) = test {
            self.remote.connect(profile, Intent::Test);
        }
        if cancel {
            self.remote.form = None;
        }
        self.delete_host_dialog(ctx);
        self.host_trust_dialog(ctx);
        let mut connect = false;
        let mut dismiss = false;
        let can_remember = self.remote.login.as_ref().is_some_and(|(profile, _)| {
            self.remote
                .profiles
                .iter()
                .any(|saved| same_identity(saved, profile))
        });
        if let Some((profile, _)) = &mut self.remote.login {
            if !can_remember {
                profile.save_password = false;
            }
            egui::Modal::new(egui::Id::new("ssh_credentials")).show(ctx, |ui| {
                ui.heading(format!("SSH credentials: {}", profile.name));
                ui.label(if matches!(profile.auth, AuthMethod::Password) {
                    "Password"
                } else {
                    "Key passphrase (leave empty for unencrypted key)"
                });
                ui.label(format!(
                    "{}@{}:{}",
                    profile.username, profile.host, profile.port
                ));
                let password = ui.add(
                    egui::TextEdit::singleline(&mut self.remote.secret)
                        .password(true)
                        .id(egui::Id::new("ssh_password")),
                );
                if std::mem::take(&mut self.remote.login_focus) {
                    password.request_focus();
                }
                if !self.remote.login_error.is_empty() { ui.colored_label(crate::theme::ERROR, &self.remote.login_error); }
                ui.add_enabled_ui(can_remember, |ui| { ui.checkbox(&mut profile.save_password, "Save password in system credential store"); });
                ui.weak(if can_remember { "macOS Keychain / Windows Credential Manager / Linux Secret Service. Passwords are never written to JSON." } else { "Save this SSH profile first to remember its password." });
                connect = ui.button("Connect").clicked()
                    || (password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                dismiss = ui.button("Cancel").clicked();
            });
        }
        if connect && let Some((profile, intent)) = self.remote.login.take() {
            let secret = std::mem::take(&mut self.remote.secret);
            let secret = Some(Secret::new(secret));
            if !self.remote.queue(Command::Connect(profile, secret, intent)) {
                self.status = "SSH queue full/stopped".into();
            }
        }
        if dismiss {
            if let Some((_, Intent::Database(id))) = &self.remote.login {
                self.database.cancel_ssh(*id, "SSH login cancelled".into());
            }
            self.remote.login = None;
            self.remote.secret.zeroize();
        }
    }
}
fn tree_row(
    ui: &mut egui::Ui,
    host: &str,
    path: &str,
    remote: &mut RemoteUi,
    loads: &mut Vec<String>,
    open: &mut Option<String>,
) {
    let key = (host.to_owned(), path.to_owned());
    let expanded = path == "." || remote.expanded.contains(&key);
    if path != "."
        && ui
            .selectable_label(
                expanded,
                format!(
                    "{} {}",
                    if expanded { "v" } else { ">" },
                    path.rsplit('/').next().unwrap_or(path)
                ),
            )
            .clicked()
    {
        if expanded {
            remote.expanded.remove(&key);
        } else {
            remote.expanded.insert(key.clone());
            loads.push(path.into());
        }
    }
    if path != "." && !remote.expanded.contains(&key) {
        return;
    }
    let Some(entries) = remote.tree.get(&key).cloned() else {
        // Explicit click retries after errors; pending requests coalesce between frames.
        if !remote.pending.contains(&key) && ui.button("Load directory").clicked() {
            loads.push(path.into());
        }
        return;
    };
    ui.indent(&key, |ui| {
        for entry in entries {
            let child = format!("{path}/{}", entry.name);
            if entry.is_dir {
                tree_row(ui, host, &child, remote, loads, open);
            } else if ui
                .selectable_label(false, &entry.name)
                .on_hover_text(format!(
                    "Size: {:?} · mode: {:?} · modified: {:?}",
                    entry.size, entry.permissions, entry.modified
                ))
                .double_clicked()
            {
                *open = Some(child);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check_masked(shape: &egui::epaint::Shape) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                assert!(!text.galley.text().contains("hidden-password"));
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    check_masked(shape);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn editing_preserves_all_forwards_and_deleted_or_changed_hosts_cannot_retarget_buffers()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        let ctx = egui::Context::default();
        let mut profile = Form::default().profile;
        profile.name = "server".into();
        profile.host = "old.example".into();
        profile.username = "user".into();
        profile.auth = AuthMethod::PrivateKeyFile {
            path: "~/.ssh/custom".into(),
        };
        profile.forwards = vec![
            PortForward {
                local_port: 15432,
                remote_host: "db".into(),
                remote_port: 5432,
            },
            PortForward {
                local_port: 13306,
                remote_host: "db".into(),
                remote_port: 3306,
            },
        ];
        let form = Form::edit(profile.clone());
        assert_eq!(form.profile.forwards, profile.forwards);
        assert_eq!(form.key_path, "~/.ssh/custom");
        assert_eq!(form.original.as_deref(), Some("server"));
        shell.remote.profiles_updated(vec![profile.clone()]);
        let cache = dir.path().join("cached");
        std::fs::write(&cache, "old text")?;
        shell.accept_remote_reply(
            Reply::Read(
                "server".into(),
                "./file".into(),
                cache.clone(),
                "old text".into(),
            ),
            &ctx,
        );
        shell.documents[0].buffer.insert(8, " changed")?;
        let mut changed = profile;
        changed.host = "new.example".into();
        shell.accept_remote_reply(Reply::ProfilesChanged(vec![changed]), &ctx);
        assert!(shell.remote.document_invalidated(&cache));
        shell.save_file();
        assert!(shell.status.contains("profile changed/deleted"));
        assert!(shell.documents[0].buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(cache)?, "old text");
        shell.remote.form = Some(form);
        shell.remote.profile_busy = true;
        shell.accept_remote_reply(Reply::Error("cannot save settings".into()), &ctx);
        assert!(!shell.remote.profile_busy);
        assert!(
            shell
                .remote
                .form
                .as_ref()
                .is_some_and(|f| f.error.contains("cannot save"))
        );
        Ok(())
    }
    #[test]
    fn trust_prompt_precedes_focused_masked_password_entry()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        let ctx = egui::Context::default();
        let profile = Form::default().profile;
        shell.accept_remote_reply(
            Reply::TrustRequired(profile.clone(), "SHA256:example".into(), Intent::Test),
            &ctx,
        );
        assert!(shell.remote.trust.is_some());
        assert!(shell.remote.login.is_none());
        let _frame = ctx.run(egui::RawInput::default(), |ctx| shell.remote_dialogs(ctx));
        shell.remote.cancel_trust();
        assert!(shell.remote.login.is_none());
        assert!(shell.remote.trust.is_none());
        shell.accept_remote_reply(Reply::Verified(profile, Intent::Test), &ctx);
        assert!(shell.remote.login.is_some());
        let _frame = ctx.run(egui::RawInput::default(), |ctx| shell.remote_dialogs(ctx));
        let input = egui::RawInput {
            events: vec![egui::Event::Text("hidden-password".into())],
            ..Default::default()
        };
        let frame = ctx.run(input, |ctx| shell.remote_dialogs(ctx));
        assert_eq!(shell.remote.secret, "hidden-password");
        for shape in frame.shapes {
            check_masked(&shape.shape);
        }
        Ok(())
    }
    #[test]
    fn remote_save_revision_race_and_offline_never_fall_back_to_local_save()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cached");
        std::fs::write(&path, "a")?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        let ctx = egui::Context::default();
        shell.accept_remote_reply(
            Reply::Read(
                "server".into(),
                "./file.rs".into(),
                path.clone(),
                "a".into(),
            ),
            &ctx,
        );
        shell.documents[0].buffer.insert(1, "b")?;
        let revision = shell.documents[0].buffer.snapshot().revision;
        shell.documents[0].buffer.insert(2, "c")?;
        shell.accept_remote_reply(Reply::Saved(path.clone(), revision), &ctx);
        assert!(shell.documents[0].buffer.is_dirty());
        shell.save_file();
        assert!(shell.status.contains("unsaved"));
        assert_eq!(std::fs::read_to_string(&path)?, "a");
        let revision = shell.documents[0].buffer.snapshot().revision;
        shell.accept_remote_reply(Reply::Saved(path, revision), &ctx);
        assert!(!shell.documents[0].buffer.is_dirty());
        shell.documents[0].buffer.insert(3, "d")?;
        shell.accept_remote_reply(Reply::Disconnected("server".into()), &ctx);
        assert!(shell.documents[0].buffer.is_dirty());
        shell.remote.form = Some(Form::default());
        let _frame = ctx.run(egui::RawInput::default(), |ctx| shell.remote_dialogs(ctx));
        assert!(shell.remote.modal_open());
        Ok(())
    }
}
