# GitLab Runner Orchestrator for Parallels Desktop (macOS)

Automatic start/stop of a pre-configured Parallels Desktop VM as a GitLab CI runner — **pay only when you need it** (no cloud bills, just local compute).

## Features

- **Automatic VM start** when pipelines are pending/running
- **Cost-aware stop** — the VM is stopped once the minimum lifetime has elapsed and no jobs remain
- **Polls all projects** — one runner for the entire GitLab instance
- **State persistence** — survives restarts without data loss
- **CSV logging** — documents all VM starts/stops with reason and duration
- **Rotating log files** — daily rotation
- **Native macOS** — no Docker, no cloud API, just `prlctl`

## Architecture

```mermaid
flowchart TB
    subgraph main [Main Loop]
        Poll[Polling Loop]
        Poll --> FetchProjects[GitLab: Fetch all projects]
        FetchProjects --> CheckPipelines[Per project: Pending/Running pipelines?]
        CheckPipelines -->|Yes + VM stopped| LogStart[CSV: Log reason]
        LogStart --> StartVM[Parallels: prlctl start]
        CheckPipelines -->|No everywhere + VM running| CheckStop[Check min lifetime]
        CheckStop -->|Elapsed| LogStop[CSV: Log usage]
        LogStop --> StopVM[Parallels: prlctl stop]
    end

    subgraph config [Config]
        ConfigToml[config/config.toml]
    end

    subgraph logs [Logging]
        CSV[logs/runner_usage.csv]
        LogFile[logs/orchestrator.log]
    end

    ConfigToml --> main
    main --> CSV
    main --> LogFile
```

## Prerequisites

- **macOS** with [Parallels Desktop](https://www.parallels.com/) installed
- A **pre-configured VM** in Parallels Desktop with:
  - A registered GitLab Runner inside (the runner must be able to pick up jobs)
  - The VM name must match what you configure in `config/config.toml`
- Rust toolchain (`cargo`) to build the orchestrator

## Quick Start

### 1. Build the binary

```bash
cargo build --release
```

### 2. Create configuration

On first start, `config/config.example.toml` is automatically created. Copy and customize it:

```bash
cp config/config.example.toml config/config.toml
# Edit config/config.toml with your GitLab credentials and VM name
```

### 3. Prepare your Parallels VM

Create a VM in Parallels Desktop and configure it with a GitLab Runner:

1. Install the GitLab Runner inside the VM:
   ```bash
   # Example for Ubuntu/Debian inside the VM:
   curl -L "https://packages.gitlab.com/install/repositories/runner/gitlab-runner/script.deb.sh" | sudo bash
   sudo apt-get install gitlab-runner
   ```

2. Register the runner:
   ```bash
   sudo gitlab-runner register \
     --url https://gitlab.example.com \
     --token YOUR_REGISTRATION_TOKEN \
     --executor shell \
     --description "parallels-runner"
   ```

3. Make sure the runner service is enabled and will start automatically when the VM boots:
   ```bash
   sudo gitlab-runner install
   sudo gitlab-runner start
   ```

4. **Stop the VM** — the orchestrator will start it when needed.

5. Note the VM name (visible in Parallels Desktop or via `prlctl list -a`).

### 4. Start the orchestrator

```bash
cargo run --release
# or
./target/release/gitlab_parallels
```

## Configuration

### config/config.toml

```toml
[gitlab]
url = "https://gitlab.example.com"
token = "glpat-xxxxxxxxxxxxxxxxxxxx"  # read_api scope
# tag_filter = ["macos", "arm64"]     # optional: only react to jobs with these tags

[parallels]
vm_name = "gitlab-runner"             # name of your pre-configured Parallels VM

[runner]
name = "flexi-runner"                 # display name for logs/CSV
min_lifetime_minutes = 20             # minimum runtime before the VM can be stopped
poll_interval_seconds = 30            # how often to check GitLab for active jobs
```

### How it works

1. The orchestrator polls your GitLab instance every `poll_interval_seconds`.
2. When a pending or running job is found, it starts the VM via `prlctl start <vm_name>`.
3. The GitLab Runner inside the VM picks up the job.
4. When no jobs remain active and the VM has been running for at least `min_lifetime_minutes`, the orchestrator stops the VM via `prlctl stop <vm_name>`.

### Listing your VMs

To find the exact name of your VM:

```bash
prlctl list -a
```

## Debug vs Release

| Feature          | Debug     | Release                          |
| ---------------- | --------- | -------------------------------- |
| Polling interval | 5s        | 30s (from config)                |
| VM stop          | Immediate | After min. lifetime              |

## Logs

- `logs/orchestrator.log` — Detailed logs (daily rotation)
- `logs/runner_usage.csv` — VM usage documentation

### CSV Format

```csv
timestamp,event,vm_name,project,pipeline_id,reason,duration_minutes
2026-01-14T10:30:00Z,START,gitlab-runner,mygroup/myproject,9876,pipeline_pending,
2026-01-14T11:15:00Z,STOP,gitlab-runner,,,all_pipelines_done,45
```

## Running as a background service (optional)

To keep the orchestrator running in the background on macOS, create a LaunchAgent:

```bash
mkdir -p ~/Library/LaunchAgents
```

Create `~/Library/LaunchAgents/tech.wascar.gitlab-parallels.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>tech.wascar.gitlab-parallels</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/gitlab_parallels/target/release/gitlab_parallels</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/path/to/gitlab_parallels</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/path/to/gitlab_parallels/logs/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/path/to/gitlab_parallels/logs/stderr.log</string>
</dict>
</plist>
```

Then load it:

```bash
launchctl load ~/Library/LaunchAgents/tech.wascar.gitlab-parallels.plist
```

## Migrating from the Scaleway version

- Rewrite `config/config.toml`: replace the `[scaleway]` section with `[parallels]` (just `vm_name`).
- Delete any existing `config/state.json`. The old Scaleway server IDs are not compatible; the orchestrator ignores an unreadable state file and starts fresh.
- Remove `config/runner.toml` — it is no longer needed. The GitLab Runner is configured inside the VM itself.
- The Docker stack (`Dockerfile`, `docker-compose.yml`) has been removed. This project is now a native macOS binary.
