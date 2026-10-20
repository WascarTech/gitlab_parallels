//! GitLab Runner Orchestrator for Parallels Desktop (macOS).
//!
//! Starts a pre-configured Parallels Desktop VM as a GitLab CI runner when
//! pipelines are pending and stops it once the minimum lifetime has elapsed.
//!
//! # How it works
//!
//! 1. Polls the GitLab API for active pipelines (pending/running)
//! 2. On active pipeline: Start the Parallels VM (if not running)
//! 3. On no active pipelines: Stop the VM (after minimum runtime)
//!
//! # Configuration
//!
//! Expects `config/config.toml` with GitLab and Parallels VM settings.
//! The VM must already exist in Parallels Desktop and be fully configured
//! with a registered GitLab Runner.

mod config;
mod csv_log;
mod gitlab;
mod parallels;
mod state;

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer};

use crate::config::Config;
use crate::csv_log::CsvLogger;
use crate::gitlab::GitLabClient;
use crate::parallels::ParallelsClient;
use crate::state::{OrchestratorState, RunnerState};

/// Default path for configuration.
const CONFIG_PATH: &str = "config/config.toml";

/// Default path for persisted state.
const STATE_PATH: &str = "config/state.json";

/// Default path for logs.
const LOG_DIR: &str = "logs";

/// Default path for example configuration.
const CONFIG_EXAMPLE_PATH: &str = "config/config.example.toml";

/// Content of the example configuration.
const CONFIG_EXAMPLE_CONTENT: &str = r#"# GitLab Runner Orchestrator for Parallels Desktop - Example Configuration
# Copy this file to config.toml and customize the values.

[gitlab]
# URL of your GitLab instance
url = "https://gitlab.example.com"
# Personal Access Token with API access (read_api scope is sufficient)
token = "glpat-xxxxxxxxxxxxxxxxxxxx"
# Optional: only spin up a runner when a pending job has one of these tags.
# Remove or leave empty to react to all pending jobs.
# tag_filter = ["macos", "my-runner-tag"]

[parallels]
# Name of the pre-configured Parallels Desktop VM.
# The VM must already exist and have a registered GitLab Runner inside.
vm_name = "gitlab-runner"

[runner]
# Display name used in logs and CSV
name = "flexi-runner"
# Minimum runtime in minutes before the VM can be stopped
min_lifetime_minutes = 20
# Polling interval in seconds
poll_interval_seconds = 30
"#;

/// Returns true if compiled in debug mode.
/// In debug mode, the VM is immediately stopped when no pipelines are active.
const fn is_debug_build() -> bool {
    cfg!(debug_assertions)
}

/// Returns true when an idle VM may be stopped.
///
/// Debug builds always stop immediately; release builds wait until the
/// minimum lifetime has elapsed.
fn should_stop(uptime_minutes: u64, min_lifetime_minutes: u32, is_debug: bool) -> bool {
    is_debug || uptime_minutes >= min_lifetime_minutes as u64
}

/// Creates the example configuration if it doesn't exist.
fn ensure_example_config() {
    if !Path::new(CONFIG_EXAMPLE_PATH).exists() {
        // Create config directory if needed
        std::fs::create_dir_all("config").ok();

        if let Err(e) = std::fs::write(CONFIG_EXAMPLE_PATH, CONFIG_EXAMPLE_CONTENT) {
            eprintln!("Warning: Could not create example configuration: {}", e);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Create example configuration if not present
    ensure_example_config();

    // Create log directory if not present
    std::fs::create_dir_all(LOG_DIR).ok();

    // File appender for rotating logs (daily)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, LOG_DIR, "orchestrator.log");

    // Initialize tracing with console + file output
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        // Console layer - compact format
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
        )
        // File layer - detailed format with timestamps
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false) // No ANSI colors in file
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
        )
        .init();

    info!("=== GitLab Runner Orchestrator for Parallels Desktop ===");
    info!("Logs are written to: {}/orchestrator.log", LOG_DIR);
    if is_debug_build() {
        warn!("DEBUG BUILD: VM will be stopped immediately when no pipelines are active!");
    }
    info!("Starting...");

    // Check if config directory exists
    if !Path::new("config").exists() {
        error!("Config directory 'config/' does not exist!");
        error!("Please create config/config.toml");
        std::process::exit(1);
    }

    // Load configuration
    let config = Config::load(CONFIG_PATH).context("Error loading configuration")?;

    // Initialize CSV logger
    let csv_logger = CsvLogger::new(LOG_DIR).context("Error initializing CSV logger")?;

    // Create API clients
    let gitlab_client = GitLabClient::new(&config.gitlab);
    let parallels_client = ParallelsClient::new(&config.parallels);

    // Load orchestrator state with persistence
    let mut state =
        OrchestratorState::with_persistence(STATE_PATH).context("Error loading state")?;

    // Verify that saved state matches the actual VM state
    verify_state_with_parallels(&parallels_client, &config.parallels.vm_name, &mut state).await?;

    // Polling interval (Debug: 5s, Release: from config)
    let poll_interval_secs = if is_debug_build() {
        5
    } else {
        config.runner.poll_interval_seconds
    };
    let poll_interval = Duration::from_secs(poll_interval_secs);

    info!("Starting polling loop (interval: {}s)", poll_interval_secs);
    info!(
        "Minimum VM runtime: {} minutes",
        config.runner.min_lifetime_minutes
    );

    // Main loop
    loop {
        if let Err(e) = orchestration_tick(
            &gitlab_client,
            &parallels_client,
            &csv_logger,
            &config,
            &mut state,
        )
        .await
        {
            error!("Error in orchestration tick: {}", e);
            // Continue on errors
        }

        sleep(poll_interval).await;
    }
}

/// Verifies that the saved state matches the actual VM state.
///
/// Possible scenarios:
/// - State says VM is running, Parallels says running → OK, keep state
/// - State says VM is running, Parallels says stopped → Clear state
/// - State says no VM, Parallels says running → Update state (emergency)
/// - Both say stopped → OK
async fn verify_state_with_parallels(
    parallels_client: &ParallelsClient,
    vm_name: &str,
    state: &mut OrchestratorState,
) -> Result<()> {
    info!("Verifying state with Parallels Desktop...");

    let vm_running = parallels_client.is_running().await?;

    match (&state.runner, vm_running) {
        // State and Parallels match - VM is running
        (Some(runner), true) => {
            info!(
                "State verified: VM {} is running for {} minutes",
                runner.vm_name,
                runner.uptime_minutes()
            );
        }

        // State says VM is running, but it's actually stopped
        (Some(runner), false) => {
            warn!(
                "State inconsistency: VM {} was running but is now stopped!",
                runner.vm_name
            );
            warn!("Clearing state...");
            state.clear_runner();
        }

        // State says no VM, but it's actually running (emergency recovery)
        (None, true) => {
            warn!(
                "VM {} is running but not in state - adopting (start time unknown)",
                vm_name
            );
            state.set_runner(RunnerState::new(vm_name.to_string()));
        }

        // All OK - VM is stopped
        (None, false) => {
            info!("No VM running - state is consistent");
        }
    }

    Ok(())
}

/// One pass of the orchestration logic.
async fn orchestration_tick(
    gitlab_client: &GitLabClient,
    parallels_client: &ParallelsClient,
    csv_logger: &CsvLogger,
    config: &Config,
    state: &mut OrchestratorState,
) -> Result<()> {
    // Query GitLab for active jobs (filtered by tag if configured)
    let active_jobs = gitlab_client
        .find_active_jobs(config.gitlab.tag_filter.as_deref())
        .await?;
    let has_active = !active_jobs.is_empty();

    if has_active {
        // There are active jobs - ensure VM is running
        if !state.has_runner() {
            // No VM running - start one
            let first_job = &active_jobs[0];
            info!(
                "Active job found: {} (ID: {}) in {}",
                first_job.job.status, first_job.job.id, first_job.project.path_with_namespace
            );

            start_runner(
                parallels_client,
                csv_logger,
                &config.parallels.vm_name,
                state,
                &first_job.project.path_with_namespace,
                first_job.job.id,
            )
            .await?;
        } else {
            // VM is already running
            if let Some(uptime) = state.runner_uptime() {
                info!(
                    "VM running ({}min), {} active job(s)",
                    uptime,
                    active_jobs.len()
                );
            }
        }
    } else {
        // No active jobs
        if state.has_runner() {
            // VM is running, but no jobs anymore - check if we should stop
            maybe_stop_runner(parallels_client, csv_logger, config, state).await?;
        } else {
            info!("No active jobs, no VM running - waiting...");
        }
    }

    Ok(())
}

/// Starts the runner VM.
async fn start_runner(
    parallels_client: &ParallelsClient,
    csv_logger: &CsvLogger,
    vm_name: &str,
    state: &mut OrchestratorState,
    project: &str,
    pipeline_id: u64,
) -> Result<()> {
    info!("Starting runner VM...");

    parallels_client
        .start()
        .await
        .context("Error starting VM")?;

    let runner_state = RunnerState::new(vm_name.to_string());
    state.set_runner(runner_state);

    if let Err(e) = csv_logger.log_start(vm_name, project, pipeline_id, "pipeline_pending") {
        warn!("Error in CSV logging: {}", e);
    }

    info!("Runner VM started and ready");
    Ok(())
}

/// Checks if the VM should be stopped and performs the stop if so.
async fn maybe_stop_runner(
    parallels_client: &ParallelsClient,
    csv_logger: &CsvLogger,
    config: &Config,
    state: &mut OrchestratorState,
) -> Result<()> {
    let runner = match &state.runner {
        Some(r) => r,
        None => return Ok(()),
    };

    let uptime = runner.uptime_minutes();
    let min_lifetime = config.runner.min_lifetime_minutes;

    if !should_stop(uptime, min_lifetime, is_debug_build()) {
        info!(
            "VM running for {}min (minimum {}min), no pipelines active - waiting...",
            uptime, min_lifetime
        );
        return Ok(());
    }

    let reason = if is_debug_build() {
        info!(
            "[DEBUG] VM running for {}min - stopping immediately (no pipelines active)",
            uptime
        );
        "debug_immediate_stop"
    } else {
        "all_pipelines_done"
    };

    stop_runner(parallels_client, csv_logger, state, reason).await
}

/// Stops the runner VM.
async fn stop_runner(
    parallels_client: &ParallelsClient,
    csv_logger: &CsvLogger,
    state: &mut OrchestratorState,
    reason: &str,
) -> Result<()> {
    let runner = match &state.runner {
        Some(r) => r,
        None => return Ok(()),
    };

    let vm_name = runner.vm_name.clone();
    let uptime = runner.uptime_minutes();

    info!("Stopping runner VM (reason: {})", reason);

    parallels_client.stop().await.context("Error stopping VM")?;

    if let Err(e) = csv_logger.log_stop(&vm_name, reason, uptime) {
        warn!("Error in CSV logging: {}", e);
    }

    state.clear_runner();

    info!("Runner VM stopped (runtime: {} minutes)", uptime);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_stop_debug_always() {
        assert!(should_stop(0, 20, true));
        assert!(should_stop(1, 999, true));
    }

    #[test]
    fn test_should_stop_before_min_lifetime() {
        assert!(!should_stop(19, 20, false));
    }

    #[test]
    fn test_should_stop_at_or_after_min_lifetime() {
        assert!(should_stop(20, 20, false));
        assert!(should_stop(65, 20, false));
    }
}
