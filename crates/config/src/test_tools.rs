//! Project test commands. Each language keeps one entry per runner.

use serde::{Deserialize, Serialize};

/// One test command the project runs inside a Docker container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestTool {
    /// Language id, currently `php`.
    #[serde(default = "default_language")]
    pub language: String,
    /// Runner id, currently `phpunit`.
    pub kind: String,
    /// Container id. Docker also accepts a name when the id is not known yet.
    pub container_id: String,
    /// Display name captured with the id.
    pub container_name: String,
    /// Binary inside the container, for example `vendor/bin/phpunit`.
    pub binary_path: String,
    /// Working directory inside the container. Empty keeps the image default.
    pub workdir: String,
    /// Arguments after the binary, as the user wants them typed.
    pub args: String,
}

fn default_language() -> String {
    "php".into()
}

/// Checks every saved test command.
///
/// # Errors
/// Returns [`crate::ConfigError::Invalid`] for an unknown runner, a duplicate, or an empty container or binary.
pub fn validate(tools: &[TestTool]) -> Result<(), crate::ConfigError> {
    let mut seen = Vec::new();
    for tool in tools {
        if tool.language != "php" || tool.kind != "phpunit" {
            return Err(crate::ConfigError::Invalid(
                "the only test runner is phpunit for php",
            ));
        }
        let key = (tool.language.as_str(), tool.kind.as_str());
        if seen.contains(&key) {
            return Err(crate::ConfigError::Invalid(
                "PHPUnit can be configured once",
            ));
        }
        seen.push(key);
        if tool.container_id.trim().is_empty() || tool.binary_path.trim().is_empty() {
            return Err(crate::ConfigError::Invalid(
                "test container id and binary path are required",
            ));
        }
        if has_break(&tool.language)
            || has_break(&tool.kind)
            || has_break(&tool.container_id)
            || has_break(&tool.container_name)
            || has_break(&tool.binary_path)
            || has_break(&tool.workdir)
            || has_break(&tool.args)
        {
            return Err(crate::ConfigError::Invalid(
                "test settings cannot contain newlines",
            ));
        }
    }
    Ok(())
}

fn has_break(text: &str) -> bool {
    text.chars()
        .any(|character| character == '\n' || character == '\r' || character == '\0')
}

#[cfg(test)]
mod tests {
    use super::TestTool;

    #[test]
    fn old_settings_default_to_no_tests_and_phpunit_roundtrips()
    -> Result<(), Box<dyn std::error::Error>> {
        let loaded = toml::from_str::<crate::Config>("tab_width = 4")?;
        assert_eq!(loaded.tests, Vec::new());
        let config = crate::Config {
            tests: vec![TestTool {
                language: "php".into(),
                kind: "phpunit".into(),
                container_id: "abc".into(),
                container_name: "php-fpm-nozhovecom".into(),
                binary_path: "vendor/bin/phpunit".into(),
                workdir: "/var/www/html".into(),
                args: "--filter PaymentTest".into(),
            }],
            ..crate::Config::default()
        };
        config.validate()?;
        assert_eq!(
            toml::from_str::<crate::Config>(&toml::to_string(&config)?)?,
            config
        );
        let mut duplicate = config;
        let extra = duplicate.tests.first().cloned().ok_or("configured test")?;
        duplicate.tests.push(extra);
        assert!(duplicate.validate().is_err());
        let mut other = crate::Config::default();
        other.tests.push(TestTool {
            language: "go".into(),
            kind: "go".into(),
            container_id: "abc".into(),
            container_name: String::new(),
            binary_path: "go".into(),
            workdir: String::new(),
            args: String::new(),
        });
        assert!(other.validate().is_err());
        Ok(())
    }
}
