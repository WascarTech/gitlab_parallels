//! Parallels Desktop client for macOS.
//!
//! Controls a pre-configured Parallels Desktop VM via the `prlctl` CLI.
//! The VM is expected to already exist and be fully configured (GitLab Runner
//! installed, registered, ready to execute jobs). This client only starts and
//! stops it.

use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, info};

use crate::config::ParallelsConfig;

/// Errors that can occur during Parallels VM operations.
#[derive(Error, Debug)]
pub enum ParallelsError {
    #[error("Failed to execute prlctl: {0}")]
    Spawn(#[from] std::io::Error),

    #[error("prlctl command failed (exit {code}): {stderr}")]
    Command { code: i32, stderr: String },
}

/// VM status as reported by `prlctl status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmStatus {
    Running,
    Stopped,
    Paused,
    Suspended,
    Unknown(String),
}

impl std::fmt::Display for VmStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmStatus::Running => write!(f, "running"),
            VmStatus::Stopped => write!(f, "stopped"),
            VmStatus::Paused => write!(f, "paused"),
            VmStatus::Suspended => write!(f, "suspended"),
            VmStatus::Unknown(s) => write!(f, "{}", s),
        }
    }
}

/// Parallels Desktop client.
pub struct ParallelsClient {
    vm_name: String,
}

impl ParallelsClient {
    /// Creates a new Parallels client for the given VM name.
    pub fn new(config: &ParallelsConfig) -> Self {
        info!("Parallels client initialized for VM: {}", config.vm_name);
        Self {
            vm_name: config.vm_name.clone(),
        }
    }

    /// Runs a `prlctl` subcommand and returns stdout on success.
    async fn run_prlctl(&self, args: &[&str]) -> Result<String, ParallelsError> {
        debug!("prlctl {}", args.join(" "));
        let output = Command::new("prlctl").args(args).output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let code = output.status.code().unwrap_or(-1);
            return Err(ParallelsError::Command { code, stderr });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Returns the current status of the configured VM.
    pub async fn get_status(&self) -> Result<VmStatus, ParallelsError> {
        let output = self.run_prlctl(&["status", &self.vm_name]).await?;
        Ok(parse_status(&output))
    }

    /// Returns true if the VM is currently running.
    pub async fn is_running(&self) -> Result<bool, ParallelsError> {
        Ok(self.get_status().await? == VmStatus::Running)
    }

    /// Starts the VM. No-op if already running.
    pub async fn start(&self) -> Result<(), ParallelsError> {
        let status = self.get_status().await?;
        if status == VmStatus::Running {
            info!("VM '{}' is already running", self.vm_name);
            return Ok(());
        }
        info!("Starting VM '{}' (current state: {})", self.vm_name, status);
        self.run_prlctl(&["start", &self.vm_name]).await?;
        info!("VM '{}' started", self.vm_name);
        Ok(())
    }

    /// Stops the VM. No-op if already stopped.
    pub async fn stop(&self) -> Result<(), ParallelsError> {
        let status = self.get_status().await?;
        if status == VmStatus::Stopped {
            info!("VM '{}' is already stopped", self.vm_name);
            return Ok(());
        }
        info!("Stopping VM '{}' (current state: {})", self.vm_name, status);
        self.run_prlctl(&["stop", &self.vm_name]).await?;
        info!("VM '{}' stopped", self.vm_name);
        Ok(())
    }
}

/// Parses the output of `prlctl status <vm>`.
///
/// Expected format:
/// `VM-UUID {vm-name} running`
/// or with additional info on following lines.
fn parse_status(output: &str) -> VmStatus {
    let lower = output.to_lowercase();
    if lower.contains(" running") || lower.ends_with("running") {
        VmStatus::Running
    } else if lower.contains(" stopped") || lower.ends_with("stopped") {
        VmStatus::Stopped
    } else if lower.contains(" paused") || lower.ends_with("paused") {
        VmStatus::Paused
    } else if lower.contains(" suspended") || lower.ends_with("suspended") {
        VmStatus::Suspended
    } else {
        VmStatus::Unknown(output.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_running() {
        let output = "11111111-2222-3333-4444-555555555555 {gitlab-runner} running";
        assert_eq!(parse_status(output), VmStatus::Running);
    }

    #[test]
    fn test_parse_status_stopped() {
        let output = "11111111-2222-3333-4444-555555555555 {gitlab-runner} stopped";
        assert_eq!(parse_status(output), VmStatus::Stopped);
    }

    #[test]
    fn test_parse_status_paused() {
        let output = "11111111-2222-3333-4444-555555555555 {gitlab-runner} paused";
        assert_eq!(parse_status(output), VmStatus::Paused);
    }

    #[test]
    fn test_parse_status_suspended() {
        let output = "11111111-2222-3333-4444-555555555555 {gitlab-runner} suspended";
        assert_eq!(parse_status(output), VmStatus::Suspended);
    }

    #[test]
    fn test_parse_status_multiline() {
        let output =
            "11111111-2222-3333-4444-555555555555 {gitlab-runner} running\n  Additional info line";
        assert_eq!(parse_status(output), VmStatus::Running);
    }

    #[test]
    fn test_parse_status_unknown() {
        let output = "some unexpected output";
        assert_eq!(
            parse_status(output),
            VmStatus::Unknown("some unexpected output".to_string())
        );
    }

    #[test]
    fn test_vm_status_display() {
        assert_eq!(VmStatus::Running.to_string(), "running");
        assert_eq!(VmStatus::Stopped.to_string(), "stopped");
        assert_eq!(VmStatus::Paused.to_string(), "paused");
        assert_eq!(VmStatus::Suspended.to_string(), "suspended");
        assert_eq!(VmStatus::Unknown("weird".to_string()).to_string(), "weird");
    }
}
