//! Project linters. Each kind is stored once.

use serde::{Deserialize, Serialize};

/// One linter the project runs inside a Docker container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinterTool {
    /// `phpstan`, `phpcs`, `psalm`, `phpmd` or `golangci-lint`.
    pub kind: String,
    /// Container id. Docker also accepts a name when the id is not known yet.
    pub container_id: String,
    /// Display name captured with the id.
    pub container_name: String,
    /// Binary inside the container, for example `vendor/bin/phpstan`.
    pub binary_path: String,
    /// Working directory inside the container. Empty keeps the image default.
    pub workdir: String,
    /// Arguments after the binary. JSON format flags are added when the tool runs.
    pub args: String,
}

/// Checks every saved linter.
///
/// # Errors
/// Returns [`crate::ConfigError::Invalid`] for an unknown kind, a duplicate, or an empty container or binary.
pub fn validate(tools: &[LinterTool]) -> Result<(), crate::ConfigError> {
    let mut seen = Vec::new();
    for tool in tools {
        if !matches!(
            tool.kind.as_str(),
            "phpstan" | "phpcs" | "psalm" | "phpmd" | "golangci-lint"
        ) {
            return Err(crate::ConfigError::Invalid(
                "linter kind must be phpstan, phpcs, psalm, phpmd, or golangci-lint",
            ));
        }
        if seen.contains(&tool.kind.as_str()) {
            return Err(crate::ConfigError::Invalid(
                "each linter can be configured once",
            ));
        }
        seen.push(tool.kind.as_str());
        if tool.container_id.trim().is_empty() || tool.binary_path.trim().is_empty() {
            return Err(crate::ConfigError::Invalid(
                "linter container id and binary path are required",
            ));
        }
        if has_break(&tool.container_id)
            || has_break(&tool.container_name)
            || has_break(&tool.binary_path)
            || has_break(&tool.workdir)
            || has_break(&tool.args)
        {
            return Err(crate::ConfigError::Invalid(
                "linter settings cannot contain newlines",
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
    use super::LinterTool;

    #[test]
    fn old_settings_default_to_no_linters_and_a_tool_roundtrips()
    -> Result<(), Box<dyn std::error::Error>> {
        let loaded = toml::from_str::<crate::Config>("tab_width = 4")?;
        assert_eq!(loaded.linters, Vec::new());
        let config = crate::Config {
            linters: vec![LinterTool {
                kind: "phpmd".into(),
                container_id: "abc".into(),
                container_name: "php-fpm-nozhovecom".into(),
                binary_path: "vendor/bin/phpmd".into(),
                workdir: "/var/www/html".into(),
                args: "classes,controllers ruleset.xml".into(),
            }],
            ..crate::Config::default()
        };
        config.validate()?;
        assert_eq!(
            toml::from_str::<crate::Config>(&toml::to_string(&config)?)?,
            config
        );
        let mut duplicate = config;
        let extra = duplicate
            .linters
            .first()
            .cloned()
            .ok_or("configured linter")?;
        duplicate.linters.push(extra);
        assert!(duplicate.validate().is_err());
        Ok(())
    }
}
