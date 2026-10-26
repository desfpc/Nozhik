//! PHP server discovery, initialization and authoritative open-document state.
use super::client::Client;
use crate::{Error, Result};
use lsp_types::{
    InitializeResult, PositionEncodingKind, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::{process::Command, sync::mpsc};

/// Built-in language contribution (independent of executable plugin RPC).
pub const PHP_PLUGIN_ID: &str = "org.nozhik.php";
/// Built-in language metadata; it does not launch executable plugin-host RPC.
pub struct LanguageDefinition {
    /// Stable contribution identifier.
    pub id: &'static str,
    /// LSP language ID.
    pub language_id: &'static str,
    /// Extensions without the leading dot.
    pub extensions: &'static [&'static str],
}
/// Default PHP language contribution.
pub const PHP: LanguageDefinition = LanguageDefinition {
    id: PHP_PLUGIN_ID,
    language_id: "php",
    extensions: &["php", "phtml"],
};
/// PHP and HTML-embedded PHP file associations.
#[must_use]
pub fn is_php(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
        PHP.extensions
            .iter()
            .any(|extension| x.eq_ignore_ascii_case(extension))
    })
}
/// Searches PATH without launching a shell, preferring phpactor.
#[must_use]
pub fn discover() -> Option<(PathBuf, Vec<String>)> {
    let paths = std::env::var_os("PATH")?;
    for (name, args) in [
        ("phpactor", vec!["language-server".into()]),
        ("intelephense", vec!["--stdio".into()]),
    ] {
        for directory in std::env::split_paths(&paths) {
            for suffix in if cfg!(windows) {
                &[".exe", ".cmd", ".bat"][..]
            } else {
                &[""][..]
            } {
                let path = directory.join(format!("{name}{suffix}"));
                if executable(&path) {
                    return Some((path, args));
                }
            }
        }
    }
    None
}
/// Owns one initialized server and document versions for a project.
pub struct LspManager {
    /// Multiplexed stdio transport.
    pub client: Client,
    /// Typed server capabilities.
    pub initialized: InitializeResult,
    documents: HashMap<Url, (i32, String)>,
}
impl LspManager {
    /// Starts and initializes a supplied command; suitable for real or mock servers.
    /// # Errors
    /// Returns process, protocol, or unsupported position encoding errors.
    pub async fn start(
        command: &mut Command,
        root: &Path,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Value>)> {
        let root = Url::from_directory_path(root)
            .map_err(|()| Error::Protocol("invalid project URI".into()))?;
        let (client, notifications) = Client::spawn(command)?;
        let value = client.request("initialize", json!({"processId":std::process::id(),"rootUri":root,"capabilities":{
            "general":{"positionEncodings":["utf-16"]},
            "textDocument":{"completion":{"completionItem":{"snippetSupport":false}},"hover":{"contentFormat":["markdown","plaintext"]},"publishDiagnostics":{"versionSupport":true}}
        },"workspaceFolders":[{"uri":root,"name":"Nozhik"}]})).await?;
        let initialized: InitializeResult = serde_json::from_value(value)?;
        if initialized
            .capabilities
            .position_encoding
            .as_ref()
            .is_some_and(|e| *e != PositionEncodingKind::UTF16)
        {
            return Err(Error::Protocol(
                "server requires unsupported position encoding".into(),
            ));
        }
        client.notify("initialized", json!({})).await?;
        Ok((
            Self {
                client,
                initialized,
                documents: HashMap::new(),
            },
            notifications,
        ))
    }
    /// Opens a document or synchronizes its latest full snapshot. Incremental-only
    /// servers receive a replacement range covering the previous document.
    /// # Errors
    /// Returns version overflow or transport errors.
    pub async fn sync(&mut self, uri: &Url, source: &str) -> Result<()> {
        if let Some((version, old)) = self.documents.get(uri) {
            if old == source {
                return Ok(());
            }
            let version = version
                .checked_add(1)
                .ok_or_else(|| Error::Protocol("LSP version overflow".into()))?;
            let kind = match &self.initialized.capabilities.text_document_sync {
                Some(TextDocumentSyncCapability::Kind(k)) => *k,
                Some(TextDocumentSyncCapability::Options(o)) => {
                    o.change.unwrap_or(TextDocumentSyncKind::NONE)
                }
                None => TextDocumentSyncKind::NONE,
            };
            let change = if kind == TextDocumentSyncKind::INCREMENTAL {
                json!({"range":{"start":{"line":0,"character":0},"end":crate::position(old,old.len())},"text":source})
            } else {
                json!({"text":source})
            };
            if kind != TextDocumentSyncKind::NONE {
                self.client.notify("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[change]})).await?;
            }
            self.documents.insert(uri.clone(), (version, source.into()));
        } else {
            self.client.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":PHP.language_id,"version":1,"text":source}})).await?;
            self.documents.insert(uri.clone(), (1, source.into()));
        }
        Ok(())
    }
    /// Current server version for rejecting delayed diagnostics.
    #[must_use]
    pub fn version(&self, uri: &Url) -> Option<i32> {
        self.documents.get(uri).map(|d| d.0)
    }
    /// Closes a document, releasing server diagnostics.
    /// # Errors
    /// Returns transport failures.
    pub async fn close(&mut self, uri: &Url) -> Result<()> {
        if self.documents.remove(uri).is_some() {
            self.client
                .notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
                .await?;
        }
        Ok(())
    }
    /// Closes every document and shuts down the child.
    pub async fn shutdown(&mut self) {
        while let Some(uri) = self.documents.keys().next().cloned() {
            let _closed = self.close(&uri).await;
        }
        self.client.shutdown().await;
    }
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
