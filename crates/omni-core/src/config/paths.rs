use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The directory name used by this Rust implementation.
///
/// We use `.claude-omni` instead of `.claude` to avoid overwriting
/// settings, credentials, sessions, and history belonging to the
/// official Claude Code (TypeScript) installation.  The original
/// `.claude` directory is only read (never written) when importing
/// settings from the official client.
pub const OMNI_DIR_NAME: &str = ".claude-omni";

/// The project-level directory name.
pub const PROJECT_DIR_NAME: &str = ".claude-omni";

/// Returns the path to the `~/.claude-omni/` directory.
pub fn claude_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(OMNI_DIR_NAME))
}

/// Returns the path to the **original** `~/.claude/` directory (read-only).
///
/// Used for one-time import of settings, credentials, and history from
/// the official Claude Code installation.  Never write to this path.
pub fn original_claude_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".claude"))
}

/// Returns `~/.claude-omni/settings.json`.
pub fn user_settings_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

/// Returns `~/.claude-omni/sessions/`.
pub fn sessions_dir() -> Result<PathBuf> {
    Ok(claude_dir()?.join("sessions"))
}

/// Walk up from `start` looking for common project-root markers.
/// Returns the directory containing the first marker found, or `start` itself
/// if no marker is found.
pub fn detect_project_root(start: &Path) -> PathBuf {
    const MARKERS: &[&str] = &[
        ".git",
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pyproject.toml",
        "Makefile",
        ".hg",
        "pom.xml",
        "build.gradle",
    ];

    let mut current = start.to_path_buf();
    loop {
        for marker in MARKERS {
            if current.join(marker).exists() {
                return current;
            }
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return start.to_path_buf(),
        }
    }
}
