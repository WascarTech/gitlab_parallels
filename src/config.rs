//! Configuration module for the GitLab Runner Orchestrator.
//!
//! Loads configuration from `config/config.toml` and provides
//! typed structs for all settings.

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use tracing::info;

/// Errors that can occur when loading configuration.
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read configuration file: {0}")]
    Read(#[from] std::io::Error),

    #[error("Failed to parse configuration: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Main configuration - contains all sub-configurations.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub gitlab: GitLabConfig,
    pub parallels: ParallelsConfig,
    pub runner: RunnerConfig,
}

/// GitLab-specific configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct GitLabConfig {
    /// URL of the GitLab instance (e.g., "https://gitlab.example.com")
    pub url: String,
    /// Personal Access Token for API authentication
    pub token: String,
    /// Optional runner tag filter - only spin up a runner when a pending job has one of these tags.
    /// If absent, all pending jobs trigger a runner (original behavior).
    pub tag_filter: Option<Vec<String>>,
}

/// Parallels Desktop configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct ParallelsConfig {
    /// Name of the pre-configured Parallels Desktop VM
    pub vm_name: String,
}

/// Runner-specific configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct RunnerConfig {
    /// Display name of the runner (used in logs and CSV)
    pub name: String,
    /// Minimum runtime in minutes before the VM can be stopped
    #[serde(default = "default_min_lifetime")]
    pub min_lifetime_minutes: u32,
    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
}

/// Default value for minimum lifetime: 20 minutes
fn default_min_lifetime() -> u32 {
    20
}

/// Default value for polling interval: 30 seconds
fn default_poll_interval() -> u64 {
    30
}

impl Config {
    /// Loads configuration from the specified file.
    ///
    /// # Arguments
    /// * `path` - Path to the config.toml file
    ///
    /// # Returns
    /// The loaded configuration or an error
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        info!("Loading configuration from: {}", path.display());

        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;

        info!("Configuration loaded successfully");
        info!("  GitLab URL: {}", config.gitlab.url);
        info!("  Parallels VM: {}", config.parallels.vm_name);
        info!("  Runner name: {}", config.runner.name);

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        assert_eq!(default_min_lifetime(), 20);
        assert_eq!(default_poll_interval(), 30);
    }

    #[test]
    fn test_parallels_config_parses() {
        let toml = r#"
vm_name = "gitlab-runner"
"#;
        let config: ParallelsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.vm_name, "gitlab-runner");
    }

    #[test]
    fn test_runner_config_defaults() {
        let toml = r#"
name = "flexi-runner"
"#;
        let config: RunnerConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.min_lifetime_minutes, 20);
        assert_eq!(config.poll_interval_seconds, 30);
    }

    #[test]
    fn test_full_config_parses() {
        let toml = r#"
[gitlab]
url = "https://gitlab.example.com"
token = "glpat-xxxxxxxxxxxxxxxxxxxx"

[parallels]
vm_name = "gitlab-runner"

[runner]
name = "flexi-runner"
min_lifetime_minutes = 45
poll_interval_seconds = 15
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.gitlab.url, "https://gitlab.example.com");
        assert_eq!(config.parallels.vm_name, "gitlab-runner");
        assert_eq!(config.runner.name, "flexi-runner");
        assert_eq!(config.runner.min_lifetime_minutes, 45);
        assert_eq!(config.runner.poll_interval_seconds, 15);
    }

    #[test]
    fn test_gitlab_tag_filter_optional() {
        let toml = r#"
url = "https://gitlab.example.com"
token = "glpat-xxx"
"#;
        let config: GitLabConfig = toml::from_str(toml).unwrap();
        assert!(config.tag_filter.is_none());
    }

    #[test]
    fn test_gitlab_tag_filter_parses() {
        let toml = r#"
url = "https://gitlab.example.com"
token = "glpat-xxx"
tag_filter = ["macos", "arm64"]
"#;
        let config: GitLabConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            config.tag_filter.as_deref(),
            Some(&["macos".to_string(), "arm64".to_string()][..])
        );
    }
}
