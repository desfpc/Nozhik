//! Shared clone/project authentication controls; secret fields are never serialized.
use nozhik_vcs::{
    clone::Authentication,
    connection::{AuthMethod, AuthProfile},
};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct AuthFields {
    pub kind: AuthMethod,
    pub username: String,
    pub secret: Zeroizing<String>,
    pub key: String,
    pub passphrase: Zeroizing<String>,
}
impl AuthFields {
    pub fn from_profile(profile: &AuthProfile) -> Self {
        Self {
            kind: profile.method,
            username: profile.username.clone(),
            key: profile.key_path.display().to_string(),
            ..Self::default()
        }
    }
    pub fn authentication(&self) -> Authentication {
        match self.kind {
            AuthMethod::Automatic => Authentication::Automatic,
            AuthMethod::Token => Authentication::Token(self.secret.clone()),
            AuthMethod::Password => Authentication::Password {
                username: self.username.clone(),
                password: self.secret.clone(),
            },
            AuthMethod::SshKey => Authentication::SshKey {
                username: if self.username.trim().is_empty() {
                    "git".into()
                } else {
                    self.username.clone()
                },
                path: self.key.clone().into(),
                passphrase: self.passphrase.clone(),
            },
        }
    }
    pub fn ready(&self) -> bool {
        match self.kind {
            AuthMethod::Automatic => true,
            AuthMethod::Token => !self.secret.is_empty(),
            AuthMethod::Password => !self.username.trim().is_empty() && !self.secret.is_empty(),
            AuthMethod::SshKey => !self.key.trim().is_empty(),
        }
    }
    pub fn clear_secrets(&mut self) {
        self.secret = Zeroizing::default();
        self.passphrase = Zeroizing::default();
    }
    pub fn show(&mut self, ui: &mut egui::Ui) -> Result<(), String> {
        ui.label("Authentication");
        ui.horizontal_wrapped(|ui| {
            for (kind, title) in [
                (AuthMethod::Automatic, "Public / system credentials"),
                (AuthMethod::Token, "Token"),
                (AuthMethod::Password, "Login / password"),
                (AuthMethod::SshKey, "SSH key"),
            ] {
                if ui.selectable_value(&mut self.kind, kind, title).changed() {
                    self.clear_secrets();
                }
            }
        });
        match self.kind {
            AuthMethod::Automatic => {
                ui.small("Uses repository/system Git credential helpers or the SSH agent.");
            }
            AuthMethod::Token => {
                ui.label("Access token (HTTPS)");
                ui.add(
                    egui::TextEdit::singleline(&mut *self.secret)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
            }
            AuthMethod::Password => {
                ui.label("Login");
                ui.text_edit_singleline(&mut self.username);
                ui.label("Password / token (HTTPS)");
                ui.add(egui::TextEdit::singleline(&mut *self.secret).password(true));
                ui.small("GitHub requires a personal access token instead of an account password.");
            }
            AuthMethod::SshKey => {
                ui.label("SSH username (default: git)");
                ui.text_edit_singleline(&mut self.username);
                ui.label("Private key path");
                let choose = ui
                    .horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.key);
                        ui.button("Choose key…").clicked()
                    })
                    .inner;
                if choose
                    && let Some(path) =
                        crate::project_dialog::pick(crate::project_dialog::Picker::File, None)?
                {
                    self.key = path.display().to_string();
                }
                ui.label("Key passphrase (optional)");
                ui.add(egui::TextEdit::singleline(&mut *self.passphrase).password(true));
            }
        }
        ui.small("Tokens, passwords and key passphrases stay in memory for this project session. Blank secret fields keep existing session credentials for the same connection.");
        Ok(())
    }
}
