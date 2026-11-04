//! One optional SFTP deployment destination per project; no credentials.
use serde::{Deserialize, Serialize};

/// Project-local deployment mapping, referencing an existing SSH profile by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpDeployment {
    /// Name of a saved project SSH connection.
    pub ssh_profile: String,
    /// Absolute POSIX project directory on the server.
    pub remote_root: String,
}
impl SftpDeployment {
    /// Checks the destination without accessing disk or network.
    /// # Errors
    /// Rejects empty profile names, control characters and ambiguous/nonabsolute roots.
    pub fn validate(&self) -> Result<(), crate::ConfigError> {
        if self.ssh_profile.trim().is_empty() || self.ssh_profile.chars().any(char::is_control) {
            return Err(crate::ConfigError::Invalid(
                "SFTP Deployment requires an SSH connection",
            ));
        }
        if !self.remote_root.starts_with('/')
            || self.remote_root.chars().any(char::is_control)
            || self.remote_root.contains('\\')
            || self.remote_root.split('/').any(|p| matches!(p, "." | ".."))
        {
            return Err(crate::ConfigError::Invalid(
                "SFTP remote root must be an absolute POSIX path without . or .. components",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deployment_roundtrips_both_formats_and_old_settings_default_to_none()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = crate::Config {
            sftp_deployment: Some(SftpDeployment {
                ssh_profile: "Production".into(),
                remote_root: "/srv/project with spaces".into(),
            }),
            ..crate::Config::default()
        };
        config.validate()?;
        assert_eq!(
            toml::from_str::<crate::Config>(&toml::to_string(&config)?)?,
            config
        );
        assert_eq!(
            serde_json::from_str::<crate::Config>(&serde_json::to_string(&config)?)?,
            config
        );
        assert!(
            toml::from_str::<crate::Config>("tab_width=4")?
                .sftp_deployment
                .is_none()
        );
        for root in ["", "relative", "/a/../b", "/a/./b", "/a\n", "C:\\project"] {
            assert!(
                SftpDeployment {
                    ssh_profile: "saved".into(),
                    remote_root: root.into()
                }
                .validate()
                .is_err()
            );
        }
        Ok(())
    }
}
