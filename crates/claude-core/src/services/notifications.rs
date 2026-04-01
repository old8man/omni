use std::io::Write;

use anyhow::Result;
use tracing::{debug, warn};

/// Notification delivery channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationChannel {
    /// Auto-detect based on the terminal emulator.
    Auto,
    /// iTerm2 OSC 9 escape sequence.
    ITerm2,
    /// iTerm2 with an additional terminal bell.
    ITerm2WithBell,
    /// Kitty OSC 99 escape sequence.
    Kitty,
    /// Ghostty OSC 777 escape sequence.
    Ghostty,
    /// Plain terminal bell (BEL character).
    TerminalBell,
    /// Notifications explicitly disabled by the user.
    Disabled,
}

impl NotificationChannel {
    /// Parse from a configuration string (matches the TS `preferredNotifChannel`).
    pub fn from_config(s: &str) -> Self {
        match s {
            "auto" => Self::Auto,
            "iterm2" => Self::ITerm2,
            "iterm2_with_bell" => Self::ITerm2WithBell,
            "kitty" => Self::Kitty,
            "ghostty" => Self::Ghostty,
            "terminal_bell" => Self::TerminalBell,
            "notifications_disabled" => Self::Disabled,
            _ => Self::Auto,
        }
    }
}

/// Options for a notification to deliver.
#[derive(Clone, Debug)]
pub struct NotificationOptions {
    pub message: String,
    pub title: Option<String>,
    pub notification_type: String,
}

const DEFAULT_TITLE: &str = "Claude Code";

/// Detect the current terminal emulator from the `TERM_PROGRAM` environment
/// variable.
pub fn detect_terminal() -> Option<String> {
    std::env::var("TERM_PROGRAM").ok()
}

/// Send a notification using the specified channel.
///
/// Returns the name of the method that was actually used, mirroring the TS
/// implementation's analytics contract.
pub fn send_notification(
    opts: &NotificationOptions,
    channel: &NotificationChannel,
) -> Result<String> {
    let title = opts.title.as_deref().unwrap_or(DEFAULT_TITLE);

    match channel {
        NotificationChannel::Auto => send_auto(opts, title),
        NotificationChannel::ITerm2 => {
            notify_iterm2(&opts.message, title)?;
            Ok("iterm2".into())
        }
        NotificationChannel::ITerm2WithBell => {
            notify_iterm2(&opts.message, title)?;
            notify_bell()?;
            Ok("iterm2_with_bell".into())
        }
        NotificationChannel::Kitty => {
            let id = generate_kitty_id();
            notify_kitty(&opts.message, title, id)?;
            Ok("kitty".into())
        }
        NotificationChannel::Ghostty => {
            notify_ghostty(&opts.message, title)?;
            Ok("ghostty".into())
        }
        NotificationChannel::TerminalBell => {
            notify_bell()?;
            Ok("terminal_bell".into())
        }
        NotificationChannel::Disabled => Ok("disabled".into()),
    }
}

/// Auto-detect the best notification method based on the running terminal.
fn send_auto(opts: &NotificationOptions, title: &str) -> Result<String> {
    let term = detect_terminal().unwrap_or_default();
    match term.as_str() {
        "Apple_Terminal" => {
            // On Apple Terminal we fall back to bell when available.
            if is_apple_terminal_bell_available() {
                notify_bell()?;
                Ok("terminal_bell".into())
            } else {
                debug!("Apple Terminal bell disabled or unavailable");
                Ok("no_method_available".into())
            }
        }
        "iTerm.app" => {
            notify_iterm2(&opts.message, title)?;
            Ok("iterm2".into())
        }
        "kitty" => {
            let id = generate_kitty_id();
            notify_kitty(&opts.message, title, id)?;
            Ok("kitty".into())
        }
        "ghostty" => {
            notify_ghostty(&opts.message, title)?;
            Ok("ghostty".into())
        }
        _ => {
            debug!(terminal = %term, "no notification method available for terminal");
            Ok("no_method_available".into())
        }
    }
}

// ── OSC escape helpers ──────────────────────────────────────────────────────

/// Send an iTerm2 notification using the OSC 9 escape sequence.
fn notify_iterm2(message: &str, _title: &str) -> Result<()> {
    // iTerm2: ESC ] 9 ; <message> ST
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "\x1b]9;{}\x1b\\", message)?;
    stdout.flush()?;
    debug!("sent iTerm2 OSC 9 notification");
    Ok(())
}

/// Send a Kitty notification using the OSC 99 escape sequence.
fn notify_kitty(message: &str, title: &str, id: u32) -> Result<()> {
    // Kitty: ESC ] 99 ; i=<id>:d=0;title ST  then  ESC ] 99 ; i=<id>:d=1:p=body;body ST
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "\x1b]99;i={}:d=0;{}\x1b\\", id, title)?;
    write!(stdout, "\x1b]99;i={}:d=1:p=body;{}\x1b\\", id, message)?;
    stdout.flush()?;
    debug!("sent Kitty OSC 99 notification");
    Ok(())
}

/// Send a Ghostty notification using the OSC 777 escape sequence.
fn notify_ghostty(message: &str, title: &str) -> Result<()> {
    // Ghostty: ESC ] 777 ; notify ; <title> ; <body> ST
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "\x1b]777;notify;{};{}\x1b\\", title, message)?;
    stdout.flush()?;
    debug!("sent Ghostty OSC 777 notification");
    Ok(())
}

/// Send a plain terminal bell (BEL character).
fn notify_bell() -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "\x07")?;
    stdout.flush()?;
    debug!("sent terminal bell");
    Ok(())
}

/// Generate a random Kitty notification identifier.
fn generate_kitty_id() -> u32 {
    rand::random::<u32>() % 10000
}

/// Best-effort check whether the Apple Terminal bell is available.
///
/// The TS implementation shells out to `osascript` and `defaults` to read the
/// Terminal.app plist. For the Rust port we keep it simple: assume the bell is
/// available unless we can positively determine otherwise. This avoids pulling
/// in a plist parser dependency for an edge case that only fires on
/// Apple_Terminal with the auto channel.
fn is_apple_terminal_bell_available() -> bool {
    let term = detect_terminal().unwrap_or_default();
    if term != "Apple_Terminal" {
        return false;
    }

    // Try osascript to get the current profile and defaults to read settings.
    let profile = std::process::Command::new("osascript")
        .args([
            "-e",
            "tell application \"Terminal\" to name of current settings of front window",
        ])
        .output();

    let profile_name = match profile {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => return true, // Can't determine — assume available
    };

    if profile_name.is_empty() {
        return true;
    }

    // Read the Terminal preferences plist via `defaults export`.
    let defaults = std::process::Command::new("defaults")
        .args(["export", "com.apple.Terminal", "-"])
        .output();

    let plist_xml = match defaults {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        _ => return true,
    };

    // Simple heuristic: look for <key>Bell</key> followed by <false/> in
    // the profile's section. A full plist parse would be more correct but
    // this covers the common case without extra dependencies.
    if let Some(profile_pos) = plist_xml.find(&profile_name) {
        let after_profile = &plist_xml[profile_pos..];
        if let Some(bell_pos) = after_profile.find("<key>Bell</key>") {
            let after_bell = &after_profile[bell_pos..];
            // Check the next value tag after the key
            if after_bell.contains("<false/>") {
                let false_pos = after_bell.find("<false/>").unwrap();
                // Make sure <false/> comes before the next <key> (it's the
                // value for Bell, not some other key).
                if let Some(next_key) = after_bell[15..].find("<key>") {
                    if false_pos < next_key + 15 {
                        warn!("Apple Terminal bell is disabled in profile '{}'", profile_name);
                        return false;
                    }
                }
            }
        }
    }

    true
}
