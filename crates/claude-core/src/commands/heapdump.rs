use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Dump process diagnostics (Rust equivalent of JS heap dump).
///
/// In the TypeScript version this creates a V8 heap snapshot. The Rust
/// equivalent collects memory statistics and process information, writing
/// a diagnostics report to the user's Desktop.
pub struct HeapdumpCommand;

#[async_trait]
impl Command for HeapdumpCommand {
    fn name(&self) -> &str {
        "heapdump"
    }

    fn description(&self) -> &str {
        "Dump process diagnostics to ~/Desktop"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let desktop = dirs::desktop_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Desktop")
        });

        // Ensure Desktop directory exists
        if let Err(e) = std::fs::create_dir_all(&desktop) {
            return CommandResult::Output(format!(
                "Failed to create output directory: {}",
                e
            ));
        }

        let timestamp = chrono_or_fallback_timestamp();
        let diag_filename = format!("claude-diagnostics-{}.txt", timestamp);
        let diag_path = desktop.join(&diag_filename);

        // Collect process diagnostics
        let pid = std::process::id();
        let mut report = String::new();

        report.push_str("Claude Code — Process Diagnostics\n");
        report.push_str("=================================\n\n");
        report.push_str(&format!("Timestamp: {}\n", timestamp));
        report.push_str(&format!("PID: {}\n", pid));
        report.push_str(&format!(
            "Version: {}\n",
            env!("CARGO_PKG_VERSION")
        ));
        report.push_str(&format!("Model: {}\n", ctx.model));

        if let Some(ref sid) = ctx.session_id {
            report.push_str(&format!("Session: {}\n", sid));
        }

        report.push_str(&format!("CWD: {}\n", ctx.cwd.display()));

        if let Some(ref root) = ctx.project_root {
            report.push_str(&format!("Project root: {}\n", root.display()));
        }

        report.push_str("\n--- Session Stats ---\n");
        report.push_str(&format!("Input tokens:  {}\n", ctx.input_tokens));
        report.push_str(&format!("Output tokens: {}\n", ctx.output_tokens));
        report.push_str(&format!("Total cost:    ${:.4}\n", ctx.total_cost));

        // System memory info (platform-specific)
        report.push_str("\n--- System Info ---\n");
        report.push_str(&format!(
            "OS: {} {}\n",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));

        // Try to get memory info from /proc on Linux or sysctl on macOS
        #[cfg(target_os = "linux")]
        {
            if let Ok(status) = std::fs::read_to_string(format!("/proc/{}/status", pid)) {
                for line in status.lines() {
                    if line.starts_with("VmRSS:")
                        || line.starts_with("VmSize:")
                        || line.starts_with("VmPeak:")
                        || line.starts_with("Threads:")
                    {
                        report.push_str(&format!("{}\n", line.trim()));
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("ps")
                .args(["-o", "rss,vsz", "-p", &pid.to_string()])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = stdout.lines().collect();
                if lines.len() >= 2 {
                    let parts: Vec<&str> = lines[1].split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(rss_kb) = parts[0].parse::<u64>() {
                            report.push_str(&format!(
                                "RSS: {} KB ({:.1} MB)\n",
                                rss_kb,
                                rss_kb as f64 / 1024.0
                            ));
                        }
                        if let Ok(vsz_kb) = parts[1].parse::<u64>() {
                            report.push_str(&format!(
                                "VSZ: {} KB ({:.1} MB)\n",
                                vsz_kb,
                                vsz_kb as f64 / 1024.0
                            ));
                        }
                    }
                }
            }
        }

        // Environment variables (filtered for safety)
        report.push_str("\n--- Relevant Environment ---\n");
        let env_keys = [
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_MAX_TURNS",
            "TERM",
            "TERM_PROGRAM",
            "SHELL",
            "LANG",
            "CLAUDE_CODE_REMOTE",
        ];
        for key in &env_keys {
            if let Ok(val) = std::env::var(key) {
                report.push_str(&format!("{}={}\n", key, val));
            }
        }

        // Write the diagnostics file
        match std::fs::write(&diag_path, &report) {
            Ok(()) => CommandResult::Output(format!(
                "Diagnostics written to:\n  {}",
                diag_path.display()
            )),
            Err(e) => CommandResult::Output(format!(
                "Failed to write diagnostics: {}",
                e
            )),
        }
    }
}

/// Generate a timestamp string suitable for filenames.
/// Uses a simple approach that does not require the `chrono` crate.
fn chrono_or_fallback_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let secs = duration.as_secs();

    // Convert to a human-readable format without chrono
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date calculation (good enough for filenames)
    let mut year = 1970u64;
    let mut remaining_days = days_since_epoch;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let days_in_months: [u64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u64;
    for &days in &days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }
    let day = remaining_days + 1;

    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
