//! Memory directory path resolution.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const CLAUDE_CONFIG_DIR: &str = ".claude";
const PROJECTS_DIR: &str = "projects";
const MEMORY_DIR: &str = "memory";
const ENTRYPOINT_NAME: &str = "MEMORY.md";

/// Whether auto-memory features are enabled.
pub fn is_auto_memory_enabled() -> bool {
    if let Ok(val) = env::var("CLAUDE_CODE_DISABLE_AUTO_MEMORY") {
        if is_truthy(&val) { return false; }
        if is_falsy(&val) { return true; }
    }
    if let Ok(val) = env::var("CLAUDE_CODE_SIMPLE") {
        if is_truthy(&val) { return false; }
    }
    true
}

/// Returns the base directory for persistent memory storage.
pub fn get_memory_base_dir() -> PathBuf {
    if let Ok(dir) = env::var("CLAUDE_CODE_REMOTE_MEMORY_DIR") {
        if !dir.is_empty() { return PathBuf::from(dir); }
    }
    get_claude_config_home()
}

fn get_claude_config_home() -> PathBuf {
    if let Ok(dir) = env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() { return PathBuf::from(dir); }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CLAUDE_CONFIG_DIR)
}

/// Returns the auto-memory directory path.
pub fn get_auto_mem_path() -> PathBuf {
    static CACHED: OnceLock<PathBuf> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            if let Ok(override_path) = env::var("CLAUDE_COWORK_MEMORY_PATH_OVERRIDE") {
                if let Some(validated) = validate_memory_path(&override_path) {
                    return validated;
                }
            }
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            compute_auto_mem_path(&cwd)
        })
        .clone()
}

/// Compute the auto-memory path for a specific working directory.
pub fn compute_auto_mem_path(cwd: &Path) -> PathBuf {
    let base = get_memory_base_dir();
    let sanitized = sanitize_path_for_key(&cwd.to_string_lossy());
    base.join(PROJECTS_DIR).join(sanitized).join(MEMORY_DIR)
}

/// Returns the auto-memory entrypoint (MEMORY.md).
pub fn get_auto_mem_entrypoint() -> PathBuf {
    get_auto_mem_path().join(ENTRYPOINT_NAME)
}

/// Returns the daily log file path for the given date components.
pub fn get_auto_mem_daily_log_path(year: i32, month: u32, day: u32) -> PathBuf {
    let yyyy = format!("{year:04}");
    let mm = format!("{month:02}");
    let dd = format!("{day:02}");
    get_auto_mem_path()
        .join("logs")
        .join(&yyyy)
        .join(&mm)
        .join(format!("{yyyy}-{mm}-{dd}.md"))
}

/// Check if an absolute path is within the auto-memory directory.
pub fn is_auto_mem_path(path: &Path) -> bool {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let auto_path = get_auto_mem_path();
    normalized.starts_with(&auto_path)
}

/// Sanitize a filesystem path into a safe directory-name key.
pub fn sanitize_path_for_key(path: &str) -> String {
    let mapped: String = path
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' => c,
            _ => '-',
        })
        .collect();
    let mut result = String::with_capacity(mapped.len());
    let mut prev_dash = false;
    for c in mapped.chars() {
        if c == '-' {
            if !prev_dash { result.push('-'); }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    result.trim_matches('-').to_string()
}

fn validate_memory_path(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() { return None; }
    let path = PathBuf::from(raw);
    if !path.is_absolute() { return None; }
    let s = path.to_string_lossy();
    if s.len() < 3 { return None; }
    if s.contains('\0') { return None; }
    if s.starts_with("\\\\") || s.starts_with("//") { return None; }
    Some(path)
}

fn is_truthy(val: &str) -> bool {
    matches!(val, "1" | "true" | "yes" | "on")
}

fn is_falsy(val: &str) -> bool {
    matches!(val, "0" | "false" | "no" | "off")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_path_basic() {
        assert_eq!(sanitize_path_for_key("/home/user/project"), "home-user-project");
    }

    #[test]
    fn test_sanitize_path_windows() {
        assert_eq!(sanitize_path_for_key("C:\\Users\\me\\proj"), "C-Users-me-proj");
    }

    #[test]
    fn test_sanitize_path_special_chars() {
        assert_eq!(sanitize_path_for_key("/path/with spaces/and@symbols"), "path-with-spaces-and-symbols");
    }

    #[test]
    fn test_sanitize_path_collapses_dashes() {
        assert_eq!(sanitize_path_for_key("///multi///slashes///"), "multi-slashes");
    }

    #[test]
    fn test_sanitize_path_preserves_dots_underscores() {
        assert_eq!(sanitize_path_for_key("my_project.v2"), "my_project.v2");
    }

    #[test]
    fn test_compute_auto_mem_path() {
        let path = compute_auto_mem_path(Path::new("/home/user/myproject"));
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("projects"));
        assert!(path_str.contains("memory"));
        assert!(path_str.contains("home-user-myproject"));
    }

    #[test]
    fn test_validate_memory_path_rejects_relative() {
        assert!(validate_memory_path("relative/path").is_none());
    }

    #[test]
    fn test_validate_memory_path_rejects_empty() {
        assert!(validate_memory_path("").is_none());
    }

    #[test]
    fn test_validate_memory_path_accepts_absolute() {
        let result = validate_memory_path("/home/user/.claude/memory");
        assert!(result.is_some());
    }

    #[test]
    fn test_daily_log_path() {
        let path = get_auto_mem_daily_log_path(2026, 3, 15);
        let path_str = path.to_string_lossy();
        assert!(path_str.ends_with("2026-03-15.md"));
        assert!(path_str.contains("logs/2026/03"));
    }
}
