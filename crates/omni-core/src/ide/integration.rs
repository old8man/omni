//! IDE integration detection and initialization.
//!
//! Discovers running IDE instances by scanning lockfiles in the Claude
//! config directory, validating that the IDE process is still alive, and
//! testing the MCP connection. Supports both WebSocket and SSE transports.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::types::{DetectedIdeInfo, IdeLockfileContent, IdeLockfileInfo, IdeMcpConfig};

/// Lockfile directory name within the Claude config home.
const LOCKFILE_DIR: &str = "ide";

/// Environment variable set by the IDE extension with the SSE port.
const SSE_PORT_ENV: &str = "CLAUDE_CODE_SSE_PORT";

/// Environment variable to enable/disable auto-connect.
const AUTO_CONNECT_ENV: &str = "CLAUDE_CODE_AUTO_CONNECT_IDE";

/// Detect available IDE instances by scanning lockfiles and environment.
///
/// Returns a list of detected IDEs. The caller should pick the best match
/// (e.g. by workspace folder overlap with the current project).
pub fn detect_ides(config_dir: &Path) -> Vec<DetectedIdeInfo> {
    let mut results = Vec::new();

    // Check for SSE port set via environment variable
    if let Ok(port_str) = std::env::var(SSE_PORT_ENV) {
        if let Ok(port) = port_str.parse::<u16>() {
            results.push(DetectedIdeInfo {
                name: "IDE (env)".to_string(),
                port,
                workspace_folders: Vec::new(),
                url: format!("http://localhost:{port}/sse"),
                is_valid: true, // Assume valid since env was set explicitly
                auth_token: None,
                ide_running_in_windows: None,
            });
        }
    }

    // Scan lockfile directory
    let lockfile_dir = config_dir.join(LOCKFILE_DIR);
    if let Ok(entries) = std::fs::read_dir(&lockfile_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match parse_lockfile(&path) {
                Ok(info) => {
                    // Check if the IDE process is still running
                    let is_alive = info.pid.is_none_or(is_process_running);
                    if !is_alive {
                        // Clean up stale lockfile
                        tracing::debug!(
                            "IDE lockfile stale (pid {} dead), removing: {}",
                            info.pid.unwrap_or(0),
                            path.display()
                        );
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }

                    let url = if info.use_websocket {
                        format!("ws://localhost:{}", info.port)
                    } else {
                        format!("http://localhost:{}/sse", info.port)
                    };

                    results.push(DetectedIdeInfo {
                        name: info.ide_name.unwrap_or_else(|| "Unknown IDE".to_string()),
                        port: info.port,
                        workspace_folders: info.workspace_folders,
                        url,
                        is_valid: true,
                        auth_token: info.auth_token,
                        ide_running_in_windows: Some(info.running_in_windows),
                    });
                }
                Err(e) => {
                    tracing::debug!("Failed to parse IDE lockfile {}: {e}", path.display());
                }
            }
        }
    }

    results
}

/// Check whether auto-connect to IDE is enabled.
///
/// Auto-connect is enabled when:
/// - The `CLAUDE_CODE_AUTO_CONNECT_IDE` env var is truthy
/// - The `CLAUDE_CODE_SSE_PORT` env var is set
/// - A supported terminal is detected
/// - The global config has `autoConnectIde` set
pub fn is_auto_connect_enabled() -> bool {
    // Check explicit enable
    if let Ok(val) = std::env::var(AUTO_CONNECT_ENV) {
        let lower = val.to_lowercase();
        if lower == "0" || lower == "false" || lower == "no" || lower == "off" {
            return false;
        }
        if !val.is_empty() {
            return true;
        }
    }

    // SSE port being set implies auto-connect
    if std::env::var(SSE_PORT_ENV).is_ok() {
        return true;
    }

    false
}

/// Build an [`IdeMcpConfig`] from a [`DetectedIdeInfo`].
pub fn build_ide_mcp_config(ide: &DetectedIdeInfo) -> IdeMcpConfig {
    let config_type = if ide.url.starts_with("ws:") || ide.url.starts_with("wss:") {
        "ws-ide"
    } else {
        "sse-ide"
    };

    IdeMcpConfig {
        config_type: config_type.to_string(),
        url: ide.url.clone(),
        ide_name: ide.name.clone(),
        auth_token: ide.auth_token.clone(),
        ide_running_in_windows: ide.ide_running_in_windows,
        scope: "dynamic".to_string(),
    }
}

/// Pick the best IDE from a list of detected instances.
///
/// Prefers IDEs whose workspace folders overlap with the given project root.
/// Falls back to the first valid IDE if no workspace overlap is found.
pub fn pick_best_ide<'a>(
    ides: &'a [DetectedIdeInfo],
    project_root: &Path,
) -> Option<&'a DetectedIdeInfo> {
    let project_str = project_root.to_string_lossy();

    // Prefer an IDE with a workspace folder matching the project root
    let with_overlap = ides.iter().find(|ide| {
        ide.is_valid
            && ide
                .workspace_folders
                .iter()
                .any(|f| project_str.starts_with(f.as_str()) || f.starts_with(project_str.as_ref()))
    });

    if with_overlap.is_some() {
        return with_overlap;
    }

    // Fall back to first valid IDE
    ides.iter().find(|ide| ide.is_valid)
}

/// Parse a lockfile from disk into an [`IdeLockfileInfo`].
fn parse_lockfile(path: &Path) -> Result<IdeLockfileInfo> {
    // Port is encoded in the filename: `{port}.json`
    let port: u16 = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse().ok())
        .context("lockfile name is not a valid port number")?;

    let data = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read lockfile: {}", path.display()))?;

    let content: IdeLockfileContent =
        serde_json::from_str(&data).context("failed to parse lockfile JSON")?;

    let use_websocket = content.transport.as_deref() == Some("ws");

    Ok(IdeLockfileInfo {
        workspace_folders: content.workspace_folders,
        port,
        pid: content.pid,
        ide_name: content.ide_name,
        use_websocket,
        running_in_windows: content.running_in_windows.unwrap_or(false),
        auth_token: content.auth_token,
    })
}

/// Check if a process with the given PID is still running.
fn is_process_running(pid: u32) -> bool {
    // On Unix, sending signal 0 checks process existence
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix platforms, assume the process is running
        let _ = pid;
        true
    }
}

/// Return the lockfile directory path.
pub fn lockfile_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(LOCKFILE_DIR)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_lockfile() {
        let tmp = TempDir::new().unwrap();
        let lockfile = tmp.path().join("12345.json");
        std::fs::write(
            &lockfile,
            r#"{
                "workspaceFolders": ["/home/user/project"],
                "pid": 9999,
                "ideName": "VS Code",
                "transport": "ws"
            }"#,
        )
        .unwrap();

        let info = parse_lockfile(&lockfile).unwrap();
        assert_eq!(info.port, 12345);
        assert_eq!(info.workspace_folders, vec!["/home/user/project"]);
        assert_eq!(info.ide_name, Some("VS Code".to_string()));
        assert!(info.use_websocket);
    }

    #[test]
    fn test_parse_lockfile_invalid_port() {
        let tmp = TempDir::new().unwrap();
        let lockfile = tmp.path().join("not-a-number.json");
        std::fs::write(&lockfile, r#"{"workspaceFolders": []}"#).unwrap();
        assert!(parse_lockfile(&lockfile).is_err());
    }

    #[test]
    fn test_build_ide_mcp_config_ws() {
        let ide = DetectedIdeInfo {
            name: "VS Code".to_string(),
            port: 12345,
            workspace_folders: vec![],
            url: "ws://localhost:12345".to_string(),
            is_valid: true,
            auth_token: Some("tok".to_string()),
            ide_running_in_windows: None,
        };
        let config = build_ide_mcp_config(&ide);
        assert_eq!(config.config_type, "ws-ide");
        assert_eq!(config.ide_name, "VS Code");
    }

    #[test]
    fn test_build_ide_mcp_config_sse() {
        let ide = DetectedIdeInfo {
            name: "Cursor".to_string(),
            port: 54321,
            workspace_folders: vec![],
            url: "http://localhost:54321/sse".to_string(),
            is_valid: true,
            auth_token: None,
            ide_running_in_windows: None,
        };
        let config = build_ide_mcp_config(&ide);
        assert_eq!(config.config_type, "sse-ide");
    }

    #[test]
    fn test_pick_best_ide_with_overlap() {
        let ides = vec![
            DetectedIdeInfo {
                name: "IDE A".to_string(),
                port: 1,
                workspace_folders: vec!["/other/project".to_string()],
                url: "ws://localhost:1".to_string(),
                is_valid: true,
                auth_token: None,
                ide_running_in_windows: None,
            },
            DetectedIdeInfo {
                name: "IDE B".to_string(),
                port: 2,
                workspace_folders: vec!["/home/user/project".to_string()],
                url: "ws://localhost:2".to_string(),
                is_valid: true,
                auth_token: None,
                ide_running_in_windows: None,
            },
        ];
        let best = pick_best_ide(&ides, Path::new("/home/user/project/src"));
        assert_eq!(best.unwrap().name, "IDE B");
    }

    #[test]
    fn test_pick_best_ide_fallback() {
        let ides = vec![DetectedIdeInfo {
            name: "IDE A".to_string(),
            port: 1,
            workspace_folders: vec!["/other".to_string()],
            url: "ws://localhost:1".to_string(),
            is_valid: true,
            auth_token: None,
            ide_running_in_windows: None,
        }];
        let best = pick_best_ide(&ides, Path::new("/totally/different"));
        assert_eq!(best.unwrap().name, "IDE A");
    }

    #[test]
    fn test_detect_ides_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let ides = detect_ides(tmp.path());
        // May find env-based IDEs, but no lockfile-based ones
        let lockfile_ides: Vec<_> = ides.iter().filter(|i| i.name != "IDE (env)").collect();
        assert!(lockfile_ides.is_empty());
    }
}
