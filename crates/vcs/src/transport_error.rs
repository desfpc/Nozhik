//! Safe transport diagnostics: never forward remote error bodies or credential-bearing URLs.
use crate::{VcsError, clone::Authentication};
use git2::{ErrorClass, ErrorCode};

pub fn credentials_ready(auth: &Authentication) -> Result<(), VcsError> {
    let message = match auth {
        Authentication::Token(token) if token.trim().is_empty() => {
            Some("Enter an access token in Settings → Git for this project session.")
        }
        Authentication::Password { username, password }
            if username.trim().is_empty() || password.is_empty() =>
        {
            Some("Enter a Git login and password/token in Settings → Git for this project session.")
        }
        _ => None,
    };
    message.map_or(Ok(()), |message| Err(VcsError::Invalid(message.into())))
}
pub fn explain(operation: &str, url: &str, error: &git2::Error) -> VcsError {
    let status = error
        .message()
        .split_once("http status code:")
        .and_then(|(_, tail)| {
            tail.trim_start()
                .split(|ch: char| !ch.is_ascii_digit())
                .next()
        })
        .and_then(|value| value.parse::<u16>().ok());
    let github = url.strip_prefix("https://github.com/").is_some();
    let reason = match (error.code(), error.class(), status) {
        (ErrorCode::Certificate, _, _) | (_, ErrorClass::Ssl, _) => {
            "TLS certificate or SSH host-key verification failed. Check system trust and host configuration; verification remains enabled."
        }
        (_, _, Some(403)) if github => {
            "HTTP 403: GitHub denied repository access. Check the token's Resource owner, selected repository and Contents permissions (read for fetch, read and write for push), plus any required organization approval."
        }
        (_, _, Some(403)) => {
            "HTTP 403: the server denied repository access. Check account/token permissions and server policy."
        }
        (_, _, Some(404)) => {
            "HTTP 404: repository not found or hidden from this account/token. Check the URL and repository access granted to the token."
        }
        (ErrorCode::Auth, _, _) | (_, _, Some(401)) => {
            "Authentication rejected (HTTP 401 or Git auth failure). Check the token/password, expiry and selected authentication method in Settings → Git."
        }
        (_, _, Some(407)) => {
            "HTTP 407: proxy authentication is required. Check the Git/network proxy configuration."
        }
        (_, _, Some(429)) => "HTTP 429: server rate limit reached. Retry later.",
        (_, _, Some(500..=599)) => {
            "The Git server returned an HTTP server error. Retry later or check its service status."
        }
        (ErrorCode::NotFastForward, _, _) => {
            "Push requires reconciliation with remote history. Fetch and review the branch before pushing again."
        }
        (_, ErrorClass::Ssh, _) => {
            "SSH connection failed. Check the SSH user, key, passphrase/agent and trusted host key."
        }
        (_, ErrorClass::Net | ErrorClass::Os, _) => {
            "Network connection failed. Check DNS, connectivity and Git proxy settings."
        }
        _ => "Check the remote URL, authentication in Settings → Git, network and server policy.",
    };
    VcsError::Invalid(format!(
        "{operation} failed. {reason} [Git: {:?}/{:?}]",
        error.code(),
        error.class()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_diagnostics_never_echo_remote_credentials() {
        for (code, class, message, expected) in [
            (
                ErrorCode::GenericError,
                ErrorClass::Http,
                "unexpected http status code: 403; DUMMY_SERVER_SECRET",
                "GitHub denied repository access",
            ),
            (
                ErrorCode::GenericError,
                ErrorClass::Http,
                "unexpected http status code: 404; DUMMY_SERVER_SECRET",
                "hidden from this account/token",
            ),
            (
                ErrorCode::Auth,
                ErrorClass::Http,
                "DUMMY_SERVER_SECRET",
                "Authentication rejected",
            ),
            (
                ErrorCode::Certificate,
                ErrorClass::Ssl,
                "DUMMY_SERVER_SECRET",
                "verification failed",
            ),
            (
                ErrorCode::GenericError,
                ErrorClass::Net,
                "DUMMY_SERVER_SECRET",
                "Network connection failed",
            ),
        ] {
            let error = git2::Error::new(code, class, message);
            let text =
                explain("Connection", "https://github.com/example/repo.git", &error).to_string();
            assert!(text.contains(expected), "{text}");
            assert!(!text.contains("DUMMY_SERVER_SECRET"));
        }
        assert!(
            credentials_ready(&Authentication::Token(zeroize::Zeroizing::new("  ".into())))
                .is_err()
        );
    }
}
