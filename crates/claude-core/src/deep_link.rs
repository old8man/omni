//! Deep Link URI Parser and Protocol Handler
//!
//! Parses `claude-cli://open` URIs. All parameters are optional:
//!   q    -- pre-fill the prompt input (not submitted)
//!   cwd  -- working directory (absolute path)
//!   repo -- owner/name slug
//!   branch -- git branch name
//!   model -- model override
//!
//! Security: values are URL-decoded, Unicode-sanitized, and rejected if they
//! contain ASCII control characters. Path traversal (../) is rejected in cwd.

use std::path::Path;

use anyhow::{bail, Context, Result};
use url::Url;

/// The custom URI scheme for deep links.
pub const DEEP_LINK_PROTOCOL: &str = "claude-cli";

/// Maximum query length -- beyond this the user cannot scan the prompt at a glance.
const MAX_QUERY_LENGTH: usize = 5000;

/// PATH_MAX on Linux is 4096. Windows MAX_PATH is 260 (32767 with long-path opt-in).
const MAX_CWD_LENGTH: usize = 4096;

/// macOS bundle identifier for the URL handler app bundle.
pub const MACOS_BUNDLE_ID: &str = "com.anthropic.claude-code-url-handler";

/// Parsed deep link action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeepLink {
    /// Pre-filled prompt query.
    pub query: Option<String>,
    /// Working directory (absolute path).
    pub cwd: Option<String>,
    /// GitHub owner/repo slug (e.g. "owner/repo").
    pub repo: Option<String>,
    /// Git branch name.
    pub branch: Option<String>,
    /// Model override.
    pub model: Option<String>,
}

/// Check if a string contains ASCII control characters (0x00-0x1F, 0x7F).
/// These can act as command separators in shells (newlines, carriage returns, etc.).
fn contains_control_chars(s: &str) -> bool {
    s.bytes().any(|b| b <= 0x1F || b == 0x7F)
}

/// GitHub owner/repo slug: alphanumerics, dots, hyphens, underscores, exactly one slash.
fn is_valid_repo_slug(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
    })
}

/// Check for directory traversal patterns in a path string.
fn contains_directory_traversal(s: &str) -> bool {
    // Normalize backslashes for cross-platform checking
    let normalized = s.replace('\\', "/");
    normalized.contains("/../")
        || normalized.ends_with("/..")
        || normalized.starts_with("../")
        || normalized == ".."
}

/// Partially sanitize Unicode by removing zero-width and other invisible characters
/// that can be used for ASCII smuggling / hidden prompt injection.
fn sanitize_unicode(s: &str) -> String {
    s.chars()
        .filter(|c| {
            // Allow normal printable characters and common whitespace
            if c.is_ascii() {
                return true;
            }
            // Filter out Unicode categories commonly used for smuggling:
            // - Zero-width characters (U+200B-U+200F, U+FEFF)
            // - Bidirectional control (U+202A-U+202E, U+2066-U+2069)
            // - Tag characters (U+E0001-U+E007F)
            // - Variation selectors used for invisible encoding (U+FE00-U+FE0F)
            let cp = *c as u32;
            !matches!(cp,
                0x200B..=0x200F |
                0x2028..=0x202F |
                0x2060..=0x206F |
                0xFEFF |
                0xFE00..=0xFE0F |
                0xE0001..=0xE007F
            )
        })
        .collect()
}

/// Parse a `claude-cli://` URI into a structured `DeepLink`.
///
/// # Errors
///
/// Returns an error if the URI is malformed, uses an unknown protocol or action,
/// or contains dangerous characters (control chars, path traversal, etc.).
pub fn parse_deep_link(uri: &str) -> Result<DeepLink> {
    // Normalize: accept with or without the trailing colon in protocol
    let normalized = if uri.starts_with(&format!("{DEEP_LINK_PROTOCOL}://")) {
        uri.to_string()
    } else if uri.starts_with(&format!("{DEEP_LINK_PROTOCOL}:")) {
        uri.replacen(
            &format!("{DEEP_LINK_PROTOCOL}:"),
            &format!("{DEEP_LINK_PROTOCOL}://"),
            1,
        )
    } else {
        bail!(
            "Invalid deep link: expected {DEEP_LINK_PROTOCOL}:// scheme, got \"{uri}\""
        );
    };

    let url = Url::parse(&normalized)
        .with_context(|| format!("Invalid deep link URL: \"{uri}\""))?;

    let host = url.host_str().unwrap_or("");
    if host != "open" {
        bail!("Unknown deep link action: \"{host}\"");
    }

    // Extract query parameters -- URL decoding is handled by url::Url
    let params: std::collections::HashMap<String, String> =
        url.query_pairs().into_owned().collect();

    let cwd = params.get("cwd").cloned();
    let repo = params.get("repo").cloned();
    let branch = params.get("branch").cloned();
    let model = params.get("model").cloned();
    let raw_query = params.get("q").cloned();

    // Validate cwd if present
    if let Some(ref cwd_val) = cwd {
        // Must be an absolute path
        let is_absolute = cwd_val.starts_with('/')
            || (cwd_val.len() >= 3
                && cwd_val.as_bytes()[0].is_ascii_alphabetic()
                && cwd_val.as_bytes()[1] == b':'
                && (cwd_val.as_bytes()[2] == b'/' || cwd_val.as_bytes()[2] == b'\\'));

        if !is_absolute {
            bail!(
                "Invalid cwd in deep link: must be an absolute path, got \"{cwd_val}\""
            );
        }

        if contains_control_chars(cwd_val) {
            bail!("Deep link cwd contains disallowed control characters");
        }

        if cwd_val.len() > MAX_CWD_LENGTH {
            bail!(
                "Deep link cwd exceeds {MAX_CWD_LENGTH} characters (got {})",
                cwd_val.len()
            );
        }

        if contains_directory_traversal(cwd_val) {
            bail!("Deep link cwd contains directory traversal (../)");
        }
    }

    // Validate repo slug format
    if let Some(ref repo_val) = repo {
        if !is_valid_repo_slug(repo_val) {
            bail!(
                "Invalid repo in deep link: expected \"owner/repo\", got \"{repo_val}\""
            );
        }
    }

    // Validate and sanitize query
    let query = if let Some(raw) = raw_query {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            let sanitized = sanitize_unicode(&trimmed);
            if contains_control_chars(&sanitized) {
                bail!("Deep link query contains disallowed control characters");
            }
            if sanitized.len() > MAX_QUERY_LENGTH {
                bail!(
                    "Deep link query exceeds {MAX_QUERY_LENGTH} characters (got {})",
                    sanitized.len()
                );
            }
            Some(sanitized)
        }
    } else {
        None
    };

    // Validate branch -- no control chars
    if let Some(ref branch_val) = branch {
        if contains_control_chars(branch_val) {
            bail!("Deep link branch contains disallowed control characters");
        }
    }

    // Validate model -- no control chars
    if let Some(ref model_val) = model {
        if contains_control_chars(model_val) {
            bail!("Deep link model contains disallowed control characters");
        }
    }

    Ok(DeepLink {
        query,
        cwd,
        repo,
        branch,
        model,
    })
}

/// Build a `claude-cli://` deep link URL from a `DeepLink` action.
pub fn build_deep_link(action: &DeepLink) -> String {
    let mut url = Url::parse(&format!("{DEEP_LINK_PROTOCOL}://open"))
        .expect("static URL should always parse");

    {
        let mut pairs = url.query_pairs_mut();
        if let Some(ref q) = action.query {
            pairs.append_pair("q", q);
        }
        if let Some(ref cwd) = action.cwd {
            pairs.append_pair("cwd", cwd);
        }
        if let Some(ref repo) = action.repo {
            pairs.append_pair("repo", repo);
        }
        if let Some(ref branch) = action.branch {
            pairs.append_pair("branch", branch);
        }
        if let Some(ref model) = action.model {
            pairs.append_pair("model", model);
        }
    }

    // Remove trailing `?` when there are no params
    let s = url.to_string();
    if s.ends_with('?') {
        s[..s.len() - 1].to_string()
    } else {
        s
    }
}

/// Register the `claude-cli://` protocol handler with the operating system.
///
/// On macOS, creates a minimal .app trampoline in `~/Applications` with
/// `CFBundleURLTypes` in its `Info.plist`. The executable is a symlink to the
/// provided `claude_path` binary (avoids signing a separate binary).
///
/// On Linux, creates a `.desktop` file and registers via `xdg-mime`.
///
/// # Errors
///
/// Returns an error if registration fails (filesystem, process exec, etc.).
pub async fn register_protocol_handler(claude_path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        register_macos(claude_path).await
    }

    #[cfg(target_os = "linux")]
    {
        register_linux(claude_path).await
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = claude_path;
        bail!("Protocol handler registration not supported on this platform")
    }
}

/// Resolve the claude binary path for protocol registration.
/// Prefers the native installer's stable symlink which survives auto-updates;
/// falls back to the current executable when the symlink is absent.
pub fn resolve_claude_path() -> Result<std::path::PathBuf> {
    let bin_name = if cfg!(windows) { "claude.exe" } else { "claude" };

    // Try ~/.local/bin/claude first (native installer stable path)
    if let Some(home) = dirs::home_dir() {
        let stable_path = home.join(".local").join("bin").join(bin_name);
        if stable_path.exists() {
            if std::fs::canonicalize(&stable_path).is_ok() {
                return Ok(stable_path);
            }
        }
    }

    // Fall back to current executable
    std::env::current_exe().context("Failed to determine current executable path")
}

#[cfg(target_os = "macos")]
async fn register_macos(claude_path: &Path) -> Result<()> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let app_dir = home
        .join("Applications")
        .join("Claude Code URL Handler.app");
    let contents_dir = app_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    let symlink_path = macos_dir.join("claude");

    // Remove any existing app bundle to start clean
    if app_dir.exists() {
        tokio::fs::remove_dir_all(&app_dir).await?;
    }

    tokio::fs::create_dir_all(&macos_dir).await?;

    let info_plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>{MACOS_BUNDLE_ID}</string>
  <key>CFBundleName</key>
  <string>Claude Code URL Handler</string>
  <key>CFBundleExecutable</key>
  <string>claude</string>
  <key>CFBundleVersion</key>
  <string>1.0</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSBackgroundOnly</key>
  <true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>Claude Code Deep Link</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>{DEEP_LINK_PROTOCOL}</string>
      </array>
    </dict>
  </array>
</dict>
</plist>"#
    );

    tokio::fs::write(contents_dir.join("Info.plist"), &info_plist).await?;

    // Symlink to the already-signed claude binary
    #[cfg(unix)]
    tokio::fs::symlink(claude_path, &symlink_path).await?;

    // Re-register with LaunchServices
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    let _ = tokio::process::Command::new(lsregister)
        .args(["-R", app_dir.to_str().unwrap_or_default()])
        .output()
        .await;

    tracing::debug!(
        "Registered {}:// protocol handler at {}",
        DEEP_LINK_PROTOCOL,
        app_dir.display()
    );

    Ok(())
}

#[cfg(target_os = "linux")]
async fn register_linux(claude_path: &Path) -> Result<()> {
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".local").join("share").to_string_lossy().into()
    });

    let desktop_dir = Path::new(&data_home).join("applications");
    tokio::fs::create_dir_all(&desktop_dir).await?;

    let desktop_file = "claude-code-url-handler.desktop";
    let desktop_path = desktop_dir.join(desktop_file);
    let claude_str = claude_path.display();

    let entry = format!(
        r#"[Desktop Entry]
Name=Claude Code URL Handler
Comment=Handle {DEEP_LINK_PROTOCOL}:// deep links for Claude Code
Exec="{claude_str}" --handle-uri %u
Type=Application
NoDisplay=true
MimeType=x-scheme-handler/{DEEP_LINK_PROTOCOL};
"#
    );

    tokio::fs::write(&desktop_path, &entry).await?;

    // Register as the default handler for the scheme
    if which::which("xdg-mime").is_ok() {
        let status = tokio::process::Command::new("xdg-mime")
            .args([
                "default",
                desktop_file,
                &format!("x-scheme-handler/{DEEP_LINK_PROTOCOL}"),
            ])
            .status()
            .await?;

        if !status.success() {
            bail!("xdg-mime exited with code {:?}", status.code());
        }
    }

    tracing::debug!(
        "Registered {}:// protocol handler at {}",
        DEEP_LINK_PROTOCOL,
        desktop_path.display()
    );

    Ok(())
}

/// Check whether the OS-level protocol handler is already registered AND
/// points at the expected `claude` binary.
pub async fn is_protocol_handler_current(claude_path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return false,
        };
        let symlink_path = home
            .join("Applications")
            .join("Claude Code URL Handler.app")
            .join("Contents")
            .join("MacOS")
            .join("claude");

        match tokio::fs::read_link(&symlink_path).await {
            Ok(target) => target == claude_path,
            Err(_) => false,
        }
    }

    #[cfg(target_os = "linux")]
    {
        let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            let home = dirs::home_dir().unwrap_or_default();
            home.join(".local").join("share").to_string_lossy().into()
        });
        let desktop_path = Path::new(&data_home)
            .join("applications")
            .join("claude-code-url-handler.desktop");

        match tokio::fs::read_to_string(&desktop_path).await {
            Ok(content) => {
                let expected_exec =
                    format!("Exec=\"{}\" --handle-uri %u", claude_path.display());
                content.contains(&expected_exec)
            }
            Err(_) => false,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = claude_path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_open() {
        let dl = parse_deep_link("claude-cli://open").unwrap();
        assert_eq!(dl, DeepLink::default());
    }

    #[test]
    fn parse_with_query() {
        let dl = parse_deep_link("claude-cli://open?q=hello+world").unwrap();
        assert_eq!(dl.query.as_deref(), Some("hello world"));
    }

    #[test]
    fn parse_with_cwd_and_repo() {
        let dl =
            parse_deep_link("claude-cli://open?cwd=/home/user/project&repo=owner/repo")
                .unwrap();
        assert_eq!(dl.cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(dl.repo.as_deref(), Some("owner/repo"));
    }

    #[test]
    fn parse_with_all_params() {
        let dl = parse_deep_link(
            "claude-cli://open?q=fix+tests&cwd=/tmp&repo=foo/bar&branch=main&model=opus",
        )
        .unwrap();
        assert_eq!(dl.query.as_deref(), Some("fix tests"));
        assert_eq!(dl.cwd.as_deref(), Some("/tmp"));
        assert_eq!(dl.repo.as_deref(), Some("foo/bar"));
        assert_eq!(dl.branch.as_deref(), Some("main"));
        assert_eq!(dl.model.as_deref(), Some("opus"));
    }

    #[test]
    fn reject_invalid_protocol() {
        assert!(parse_deep_link("http://open?q=hello").is_err());
    }

    #[test]
    fn reject_unknown_action() {
        assert!(parse_deep_link("claude-cli://close").is_err());
    }

    #[test]
    fn reject_relative_cwd() {
        assert!(parse_deep_link("claude-cli://open?cwd=relative/path").is_err());
    }

    #[test]
    fn reject_control_chars_in_cwd() {
        assert!(parse_deep_link("claude-cli://open?cwd=/home/user%0a/evil").is_err());
    }

    #[test]
    fn reject_cwd_too_long() {
        let long_path = format!("/{}", "a".repeat(MAX_CWD_LENGTH));
        let uri = format!("claude-cli://open?cwd={long_path}");
        assert!(parse_deep_link(&uri).is_err());
    }

    #[test]
    fn reject_directory_traversal() {
        assert!(parse_deep_link("claude-cli://open?cwd=/home/../etc/passwd").is_err());
    }

    #[test]
    fn reject_invalid_repo_slug() {
        assert!(parse_deep_link("claude-cli://open?repo=not-a-repo").is_err());
        assert!(parse_deep_link("claude-cli://open?repo=a/b/c").is_err());
        assert!(parse_deep_link("claude-cli://open?repo=has%20space/repo").is_err());
    }

    #[test]
    fn reject_control_chars_in_query() {
        assert!(parse_deep_link("claude-cli://open?q=hello%0aworld").is_err());
    }

    #[test]
    fn reject_query_too_long() {
        let long_q = "a".repeat(MAX_QUERY_LENGTH + 1);
        let uri = format!("claude-cli://open?q={long_q}");
        assert!(parse_deep_link(&uri).is_err());
    }

    #[test]
    fn build_roundtrips() {
        let dl = DeepLink {
            query: Some("hello world".into()),
            cwd: Some("/home/user".into()),
            repo: Some("owner/repo".into()),
            branch: None,
            model: None,
        };
        let built = build_deep_link(&dl);
        let parsed = parse_deep_link(&built).unwrap();
        assert_eq!(parsed, dl);
    }

    #[test]
    fn build_empty_deep_link() {
        let dl = DeepLink::default();
        let built = build_deep_link(&dl);
        assert_eq!(built, "claude-cli://open");
    }

    #[test]
    fn normalize_protocol_without_double_slash() {
        let dl = parse_deep_link("claude-cli:open?q=test").unwrap();
        assert_eq!(dl.query.as_deref(), Some("test"));
    }

    #[test]
    fn unicode_sanitization_removes_zero_width() {
        // U+200B zero-width space embedded in query
        let dl = parse_deep_link("claude-cli://open?q=he%E2%80%8Bllo").unwrap();
        assert_eq!(dl.query.as_deref(), Some("hello"));
    }

    #[test]
    fn valid_repo_slugs() {
        assert!(is_valid_repo_slug("owner/repo"));
        assert!(is_valid_repo_slug("my-org/my.repo"));
        assert!(is_valid_repo_slug("a_b/c_d"));
    }

    #[test]
    fn invalid_repo_slugs() {
        assert!(!is_valid_repo_slug("noslash"));
        assert!(!is_valid_repo_slug("a/b/c"));
        assert!(!is_valid_repo_slug("/leadingslash"));
        assert!(!is_valid_repo_slug("trailingslash/"));
        assert!(!is_valid_repo_slug("has spaces/repo"));
    }

    #[test]
    fn windows_absolute_path_accepted() {
        let dl = parse_deep_link("claude-cli://open?cwd=C%3A%5CUsers%5Ctest").unwrap();
        assert_eq!(dl.cwd.as_deref(), Some("C:\\Users\\test"));
    }
}
