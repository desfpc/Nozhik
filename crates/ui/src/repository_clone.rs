//! Session-only GitHub account and clone dialog; I/O runs outside the UI thread.
use nozhik_vcs::clone::Authentication;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum AuthKind {
    #[default]
    Automatic,
    Token,
    Password,
    Ssh,
}
#[derive(serde::Deserialize)]
struct Account {
    login: String,
}
#[derive(Clone, serde::Deserialize)]
struct Repository {
    full_name: String,
    private: bool,
}
enum Reply {
    Account(String, Vec<Repository>),
    Cloned(PathBuf),
}

#[derive(Default)]
pub struct CloneDialog {
    pub open: bool,
    github: bool,
    url: String,
    destination: String,
    auth: AuthKind,
    username: String,
    secret: Zeroizing<String>,
    key: String,
    passphrase: Zeroizing<String>,
    token: Zeroizing<String>,
    account: Option<String>,
    repos: Vec<Repository>,
    filter: String,
    selected: Option<String>,
    error: Option<String>,
    pending: Option<mpsc::Receiver<Result<Reply, String>>>,
    cancel: Arc<AtomicBool>,
}

async fn repositories_at(
    base: &str,
    token: &str,
    cancel: &AtomicBool,
) -> Result<(String, Vec<Repository>), String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("Nozhik/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Cannot initialize HTTPS")?;
    let request = |path: &str| {
        client
            .get(format!("{base}{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    };
    let account: Account = request("/user")
        .send()
        .await
        .map_err(|_| "Cannot reach GitHub")?
        .error_for_status()
        .map_err(|_| "GitHub rejected the token. Check token permissions and expiry.")?
        .json()
        .await
        .map_err(|_| "Invalid GitHub account response")?;
    let mut repos = Vec::new();
    for page in 1..=100 {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let batch: Vec<Repository> = request(&format!("/user/repos?per_page=100&sort=updated&affiliation=owner,collaborator,organization_member&page={page}"))
            .send().await.map_err(|_| "Cannot reach GitHub")?
            .error_for_status().map_err(|_| "Cannot list repositories. Check token permissions or API rate limit.")?
            .json().await.map_err(|_| "Invalid GitHub repositories response")?;
        let end = batch.len() < 100;
        repos.extend(batch);
        if end {
            return Ok((account.login, repos));
        }
    }
    Err("More than 10,000 repositories; use the URL tab to clone.".into())
}

impl CloneDialog {
    pub const fn busy(&self) -> bool {
        self.pending.is_some()
    }
    fn start(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(Arc<AtomicBool>) -> Result<Reply, String> + Send + 'static,
    ) {
        let (send, receive) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let wake = ctx.clone();
        self.pending = Some(receive);
        self.error = None;
        std::thread::spawn(move || {
            let result = task(cancel);
            let _sent = send.send(result);
            wake.request_repaint();
        });
    }
    pub fn tick(&mut self) -> Option<PathBuf> {
        let result = self.pending.as_ref()?.try_recv();
        let result = match result {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("Repository worker stopped".into()),
        };
        self.pending = None;
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        match result {
            Ok(Reply::Cloned(path)) => {
                self.clear_secrets();
                self.open = false;
                return Some(path);
            }
            Ok(Reply::Account(login, repos)) => {
                self.account = Some(login);
                self.repos = repos;
                self.selected = None;
            }
            Err(error) => self.error = Some(error),
        }
        None
    }
    pub fn leave_launcher(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.open = false;
        self.clear_secrets();
    }
    fn clear_secrets(&mut self) {
        self.secret = Zeroizing::default();
        self.passphrase = Zeroizing::default();
        self.token = Zeroizing::default();
        self.account = None;
        self.repos.clear();
        self.selected = None;
    }
    fn authentication(&self) -> Authentication {
        if self.github {
            return Authentication::Token(self.token.clone());
        }
        match self.auth {
            AuthKind::Automatic => Authentication::Automatic,
            AuthKind::Token => Authentication::Token(self.secret.clone()),
            AuthKind::Password => Authentication::Password {
                username: self.username.clone(),
                password: self.secret.clone(),
            },
            AuthKind::Ssh => Authentication::SshKey {
                username: if self.username.is_empty() {
                    "git".into()
                } else {
                    self.username.clone()
                },
                path: PathBuf::from(&self.key),
                passphrase: self.passphrase.clone(),
            },
        }
    }
    fn can_clone(&self) -> bool {
        if self.destination.trim().is_empty() {
            return false;
        }
        if self.github {
            return self.account.is_some() && self.selected.is_some() && !self.token.is_empty();
        }
        if nozhik_vcs::clone::normalize_url(&self.url).is_err() {
            return false;
        }
        match self.auth {
            AuthKind::Automatic => true,
            AuthKind::Token => !self.secret.is_empty(),
            AuthKind::Password => !self.username.trim().is_empty() && !self.secret.is_empty(),
            AuthKind::Ssh => !self.key.trim().is_empty(),
        }
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut connect = false;
        let mut clone = false;
        let mut dismiss = false;
        let busy = self.busy();
        egui::Modal::new(egui::Id::new("clone_repository")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.heading("Clone a Repository");
            ui.add_space(12.0);
            egui::ScrollArea::vertical()
                .id_salt("clone_form")
                .max_height((ctx.screen_rect().height() - 150.0).max(120.0))
                .show(ui, |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.github, true, "GitHub.com");
                            ui.selectable_value(&mut self.github, false, "URL");
                        });
                        ui.separator();
                        if self.github {
                            connect = self.github_form(ui);
                        } else {
                            self.url_form(ui);
                        }
                        ui.add_space(10.0);
                        ui.label("Local destination (new project directory)");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.destination)
                                    .desired_width(485.0),
                            );
                            if ui.button("Choose…").clicked() {
                                match crate::project_dialog::pick(
                                    crate::project_dialog::Picker::Folder,
                                    None,
                                ) {
                                    Ok(Some(path)) => {
                                        let remote = self.selected.as_deref().unwrap_or(&self.url);
                                        let name = remote
                                            .trim_end_matches('/')
                                            .rsplit(['/', ':'])
                                            .next()
                                            .unwrap_or("project")
                                            .trim_end_matches(".git");
                                        self.destination = path
                                            .join(if name.is_empty() { "project" } else { name })
                                            .display()
                                            .to_string();
                                    }
                                    Ok(None) => {}
                                    Err(error) => self.error = Some(error),
                                }
                            }
                        });
                    });
                });
            if let Some(error) = &self.error {
                ui.colored_label(crate::theme::ERROR, error);
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Working…");
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                dismiss = ui
                    .button(if busy { "Cancel operation" } else { "Cancel" })
                    .clicked();
                clone = ui
                    .add_enabled(!busy && self.can_clone(), egui::Button::new("Clone"))
                    .clicked();
            });
        });
        if dismiss {
            self.cancel.store(true, Ordering::Relaxed);
            self.clear_secrets();
            self.open = false;
        }
        if connect {
            self.connect(ctx);
        }
        if clone {
            self.start_clone(ctx);
        }
    }
    fn start_clone(&mut self, ctx: &egui::Context) {
        let remote = if self.github {
            format!(
                "https://github.com/{}",
                self.selected.as_deref().unwrap_or_default()
            )
        } else {
            self.url.clone()
        };
        let destination = PathBuf::from(self.destination.trim());
        let auth = self.authentication();
        self.start(ctx, move |cancel| {
            nozhik_vcs::clone::repository(&remote, &destination, &auth, &cancel)
                .map(Reply::Cloned)
                .map_err(|error| error.to_string())
        });
    }
    fn connect(&mut self, ctx: &egui::Context) {
        let token = self.token.clone();
        self.start(ctx, move |cancel| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Cannot start GitHub worker")?;
            let (login, repos) =
                runtime.block_on(repositories_at("https://api.github.com", &token, &cancel))?;
            Ok(Reply::Account(login, repos))
        });
    }
    fn github_form(&mut self, ui: &mut egui::Ui) -> bool {
        ui.label("Personal access token");
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.token)
                    .password(true)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            self.account = None;
            self.selected = None;
            self.repos.clear();
        }
        ui.small("Token stays in memory for this dialog. Grant access to the repositories you want to clone.");
        let connect = ui
            .add_enabled(
                !self.token.is_empty(),
                egui::Button::new(if self.account.is_some() {
                    "Refresh repositories"
                } else {
                    "Connect GitHub account"
                }),
            )
            .clicked();
        ui.hyperlink_to(
            "Create a token on GitHub",
            "https://github.com/settings/tokens",
        );
        if let Some(login) = &self.account {
            ui.label(format!("Connected as {login}"));
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter your repositories")
                .desired_width(f32::INFINITY),
        );
        let filter = self.filter.to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("github_repos")
            .max_height(200.0)
            .show(ui, |ui| {
                if self.account.is_some() && self.repos.is_empty() {
                    ui.weak("No repositories available to this token");
                }
                for repo in &self.repos {
                    if !repo.full_name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let label = format!(
                        "{}{}",
                        repo.full_name,
                        if repo.private { "  (private)" } else { "" }
                    );
                    if ui
                        .selectable_label(self.selected.as_ref() == Some(&repo.full_name), label)
                        .clicked()
                    {
                        self.selected = Some(repo.full_name.clone());
                    }
                }
            });
        connect
    }
    fn url_form(&mut self, ui: &mut egui::Ui) {
        ui.label("Repository URL or GitHub owner/repository");
        ui.add(egui::TextEdit::singleline(&mut self.url).desired_width(f32::INFINITY));
        ui.label("Authentication");
        ui.horizontal(|ui| {
            for (kind, title) in [
                (AuthKind::Automatic, "Public / system credentials"),
                (AuthKind::Token, "Token"),
                (AuthKind::Password, "Login / password"),
                (AuthKind::Ssh, "SSH key"),
            ] {
                ui.selectable_value(&mut self.auth, kind, title);
            }
        });
        match self.auth {
            AuthKind::Automatic => {
                ui.small(
                    "Uses a public connection, SSH agent, or configured Git credential helper.",
                );
            }
            AuthKind::Token => {
                ui.label("Access token (HTTPS)");
                ui.add(
                    egui::TextEdit::singleline(&mut *self.secret)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
            }
            AuthKind::Password => {
                ui.label("Login");
                ui.text_edit_singleline(&mut self.username);
                ui.label("Password / token (HTTPS)");
                ui.add(egui::TextEdit::singleline(&mut *self.secret).password(true));
                ui.small("GitHub requires a personal access token instead of an account password.");
            }
            AuthKind::Ssh => {
                ui.label("SSH username (default: git)");
                ui.text_edit_singleline(&mut self.username);
                ui.label("Private key path");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.key);
                    if ui.button("Choose key…").clicked() {
                        match crate::project_dialog::pick(crate::project_dialog::Picker::File, None)
                        {
                            Ok(Some(path)) => self.key = path.display().to_string(),
                            Ok(None) => {}
                            Err(error) => self.error = Some(error),
                        }
                    }
                });
                ui.label("Key passphrase (optional)");
                ui.add(egui::TextEdit::singleline(&mut *self.passphrase).password(true));
            }
        }
    }
}
impl Drop for CloneDialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn github_checks_account_paginates_and_sends_token() -> Result<(), Box<dyn std::error::Error>> {
        let server = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = server.local_addr()?;
        let worker = std::thread::spawn(move || -> Result<(), String> {
            for page in 0..3 {
                let (mut connection, _) = server.accept().map_err(|e| e.to_string())?;
                connection
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !request.windows(4).any(|b| b == b"\r\n\r\n") {
                    let size = connection.read(&mut buffer).map_err(|e| e.to_string())?;
                    if size == 0 {
                        return Err("Early EOF".into());
                    }
                    request.extend_from_slice(&buffer[..size]);
                }
                let request = String::from_utf8(request).map_err(|e| e.to_string())?;
                assert!(
                    request
                        .to_lowercase()
                        .contains("authorization: bearer session-token")
                );
                assert!(
                    request
                        .to_lowercase()
                        .contains(concat!("user-agent: nozhik/", env!("CARGO_PKG_VERSION")))
                );
                let body = if page == 0 {
                    assert!(request.starts_with("GET /user HTTP/1.1"));
                    r#"{"login":"test-account"}"#.into()
                } else {
                    assert!(request.contains(&format!("page={page} ")));
                    let count = if page == 1 { 100 } else { 1 };
                    serde_json::to_string(&(0..count).map(|i| serde_json::json!({"full_name": format!("owner/repo-{page}-{i}"), "private": true})).collect::<Vec<_>>()).map_err(|e| e.to_string())?
                };
                write!(connection, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
            }
            Ok(())
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (login, repos) = runtime.block_on(repositories_at(
            &format!("http://{address}"),
            "session-token",
            &AtomicBool::new(false),
        ))?;
        assert_eq!(login, "test-account");
        assert_eq!(repos.len(), 101);
        worker.join().map_err(|_| "Mock server panicked")??;
        Ok(())
    }

    #[test]
    fn clone_form_gates_credentials_and_ignores_cancelled_reply() {
        let mut dialog = CloneDialog::default();
        dialog.url.push_str("owner/repository");
        dialog.destination.push_str("/tmp/new-project");
        assert!(dialog.can_clone());
        dialog.auth = AuthKind::Password;
        assert!(!dialog.can_clone());
        dialog.username = "login".into();
        dialog.secret = Zeroizing::new("password".into());
        assert!(dialog.can_clone());
        assert!(
            matches!(dialog.authentication(), Authentication::Password { username, password } if username == "login" && password.as_str() == "password")
        );
        dialog.auth = AuthKind::Ssh;
        assert!(!dialog.can_clone());
        dialog.key = "/tmp/key".into();
        assert!(dialog.can_clone());
        dialog.github = true;
        assert!(!dialog.can_clone());
        dialog.account = Some("login".into());
        dialog.selected = Some("owner/repository".into());
        dialog.token = Zeroizing::new("token".into());
        assert!(dialog.can_clone());
        let (send, receive) = mpsc::channel();
        dialog.pending = Some(receive);
        dialog.cancel.store(true, Ordering::Relaxed);
        assert!(
            send.send(Ok(Reply::Cloned(PathBuf::from("/tmp/cancelled"))))
                .is_ok()
        );
        assert!(dialog.tick().is_none());
        dialog.clear_secrets();
        assert!(dialog.token.is_empty() && dialog.secret.is_empty() && dialog.account.is_none());
    }
}
