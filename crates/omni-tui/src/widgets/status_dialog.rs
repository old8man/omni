//! Full-featured status dialog overlay for `/status`.
//!
//! Displays comprehensive diagnostics matching the original claude-code Status tab:
//! system info, authentication, configuration, API connectivity, MCP servers, git info,
//! and environment variables — all inside a ratatui dialog with scrolling.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{StatefulWidget, Widget};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState};

/// A section within the status dialog.
struct Section {
    title: &'static str,
    rows: Vec<StatusRow>,
}

/// A single key→value row in a section.
struct StatusRow {
    key: String,
    value: String,
    value_color: Color,
}

impl StatusRow {
    fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { key: key.into(), value: value.into(), value_color: Color::White }
    }

    fn colored(key: impl Into<String>, value: impl Into<String>, color: Color) -> Self {
        Self { key: key.into(), value: value.into(), value_color: color }
    }
}

/// The key actions that the status dialog can produce.
pub enum StatusDialogAction {
    /// Event was consumed — nothing more to do.
    Consumed,
    /// Close the dialog.
    Close,
}

/// State for the status dialog overlay.
pub struct StatusDialog {
    /// All lines of content rendered inside the dialog.
    lines: Vec<Line<'static>>,
    /// Current scroll offset (rows from top).
    scroll: u16,
}

impl StatusDialog {
    /// Build the dialog by collecting all available diagnostics.
    pub fn new(ctx: &StatusDialogContext) -> Self {
        let sections = build_sections(ctx);
        let lines = render_sections(sections);
        Self { scroll: 0, lines }
    }

    /// Handle a keypress. Returns what the caller should do.
    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> StatusDialogAction {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Esc | KeyCode::Char('q') => StatusDialogAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                StatusDialogAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                StatusDialogAction::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(20);
                StatusDialogAction::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(20);
                StatusDialogAction::Consumed
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
                StatusDialogAction::Consumed
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.scroll = self.lines.len().saturating_sub(1) as u16;
                StatusDialogAction::Consumed
            }
            _ => StatusDialogAction::Consumed,
        }
    }

    /// Total number of content lines.
    fn total_lines(&self) -> usize {
        self.lines.len()
    }
}

impl Widget for &StatusDialog {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Dialog size: 90% width, up to 40 rows tall
        let width = (area.width * 90 / 100).max(60).min(area.width);
        let height = (area.height * 85 / 100).max(16).min(area.height);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let dialog_area = Rect::new(x, y, width, height);

        // Clear the background
        Clear.render(dialog_area, buf);

        let block = Block::default()
            .title(Span::styled(
                " Status  [↑/↓ scroll · Esc close] ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Rgb(18, 18, 28)));

        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        // Available rows for content
        let visible_rows = inner.height as usize;
        let total = self.total_lines();

        // Clamp scroll
        let max_scroll = total.saturating_sub(visible_rows) as u16;
        let scroll = self.scroll.min(max_scroll);
        let start = scroll as usize;
        let end = (start + visible_rows).min(total);

        // Render visible lines
        for (i, line) in self.lines[start..end].iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x, y, line, inner.width);
        }

        // Scrollbar on the right edge of inner area
        if total > visible_rows {
            let mut scrollbar_state = ScrollbarState::new(total)
                .position(start)
                .viewport_content_length(visible_rows);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .track_symbol(Some("│"))
                .thumb_symbol("█");
            // Render scrollbar along the right side of the dialog border
            let scrollbar_area = Rect::new(
                dialog_area.x + dialog_area.width - 1,
                dialog_area.y + 1,
                1,
                dialog_area.height.saturating_sub(2),
            );
            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

// ── Context ──────────────────────────────────────────────────────────────────

/// All the data the dialog needs to render itself.
/// Built by the App before opening the dialog.
pub struct StatusDialogContext {
    pub model: String,
    pub session_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_cost: f64,
    pub turn_count: u64,
    pub session_duration_ms: u64,
    pub api_duration_ms: u64,
    pub tool_duration_ms: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub vim_mode: bool,
    pub plan_mode: bool,
    pub fast_mode: bool,
    pub brief_mode: bool,
    pub model_usage: Vec<(String, u64, u64, u64, u64, f64)>,
    pub cwd: std::path::PathBuf,
}

// ── Section builders ─────────────────────────────────────────────────────────

fn build_sections(ctx: &StatusDialogContext) -> Vec<Section> {
    let mut sections = Vec::new();

    // ── System info ──────────────────────────────────────────────────────────
    {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let rust_version = env!("CARGO_PKG_VERSION");

        let mut rows = vec![
            StatusRow::new("OS", format!("{} ({})", os, arch)),
            StatusRow::new("Rust TUI version", rust_version),
            StatusRow::new("Working directory", ctx.cwd.display().to_string()),
        ];

        // Config dir
        if let Ok(dir) = omni_core::config::paths::claude_dir() {
            rows.push(StatusRow::colored(
                "Config dir",
                dir.display().to_string(),
                Color::DarkGray,
            ));
        }

        // Sessions dir
        if let Ok(dir) = omni_core::config::paths::sessions_dir() {
            rows.push(StatusRow::colored(
                "Sessions dir",
                dir.display().to_string(),
                Color::DarkGray,
            ));
        }

        sections.push(Section { title: "System", rows });
    }

    // ── Authentication ───────────────────────────────────────────────────────
    {
        let mut rows = Vec::new();

        match omni_core::auth::profiles::get_active_profile() {
            Some(profile) => {
                rows.push(StatusRow::colored(
                    "Logged in as",
                    profile.email.clone(),
                    Color::Green,
                ));
                let sub = capitalize_first(&profile.subscription_type);
                rows.push(StatusRow::colored("Subscription", sub, Color::Cyan));
                let auth_type = if profile.credentials.api_key.is_some() {
                    "API Key"
                } else {
                    "OAuth"
                };
                rows.push(StatusRow::new("Auth method", auth_type));

                if let Some(exp) = profile.credentials.expires_at {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    if exp > now_ms {
                        let mins = (exp - now_ms) / 60_000;
                        let (label, color) = if mins < 10 {
                            (format!("expires in {}m (refresh soon)", mins), Color::Yellow)
                        } else if mins < 60 {
                            (format!("expires in {}m", mins), Color::Green)
                        } else {
                            (format!("expires in {}h {}m", mins / 60, mins % 60), Color::Green)
                        };
                        rows.push(StatusRow::colored("Token expiry", label, color));
                    } else {
                        rows.push(StatusRow::colored(
                            "Token expiry",
                            "EXPIRED — run /login to refresh",
                            Color::Red,
                        ));
                    }
                }

                if !profile.credentials.scopes.is_empty() {
                    rows.push(StatusRow::colored(
                        "Scopes",
                        profile.credentials.scopes.join(", "),
                        Color::DarkGray,
                    ));
                }
            }
            None => {
                // Check for ANTHROPIC_API_KEY env var
                if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                    let masked = mask_key(&key);
                    rows.push(StatusRow::colored(
                        "Auth",
                        format!("ANTHROPIC_API_KEY ({})", masked),
                        Color::Yellow,
                    ));
                } else {
                    rows.push(StatusRow::colored(
                        "Auth",
                        "Not logged in — run /login",
                        Color::Red,
                    ));
                }
            }
        }

        // List all profiles
        let profiles = omni_core::auth::profiles::list_profiles();
        if !profiles.is_empty() {
            let active_name = omni_core::auth::profiles::get_active_profile_name();
            for p in &profiles {
                let status = p.status_label(active_name.as_deref());
                let color = match status {
                    "active" => Color::Green,
                    "valid" => Color::White,
                    _ => Color::Red,
                };
                let indicator = match status {
                    "active" => "● ",
                    "valid" => "○ ",
                    _ => "✕ ",
                };
                rows.push(StatusRow::colored(
                    format!("  Profile"),
                    format!("{}{} [{}]", indicator, p.display_name(), status),
                    color,
                ));
            }
        }

        sections.push(Section { title: "Authentication", rows });
    }

    // ── Session ──────────────────────────────────────────────────────────────
    {
        let dur_s = ctx.session_duration_ms / 1000;
        let duration_str = if dur_s < 60 {
            format!("{}s", dur_s)
        } else {
            format!("{}m {}s", dur_s / 60, dur_s % 60)
        };

        let rows = vec![
            StatusRow::new("Session ID", ctx.session_id.clone().unwrap_or_else(|| "(none)".into())),
            StatusRow::new("Model", ctx.model.clone()),
            StatusRow::new("Turns", ctx.turn_count.to_string()),
            StatusRow::new("Duration", duration_str),
        ];
        sections.push(Section { title: "Session", rows });
    }

    // ── Token usage ──────────────────────────────────────────────────────────
    {
        let total_in = ctx.input_tokens;
        let total_out = ctx.output_tokens;
        let cache_r = ctx.cache_read_tokens;
        let cache_w = ctx.cache_write_tokens;
        let total_all = total_in + total_out;

        let rows = vec![
            StatusRow::new("Input tokens", fmt_num(total_in)),
            StatusRow::new("Output tokens", fmt_num(total_out)),
            StatusRow::new("Cache read tokens", fmt_num(cache_r)),
            StatusRow::new("Cache write tokens", fmt_num(cache_w)),
            StatusRow::new("Total tokens", fmt_num(total_all)),
            StatusRow::colored("Total cost", fmt_cost(ctx.total_cost), Color::Green),
            StatusRow::new("API time", format!("{:.1}s", ctx.api_duration_ms as f64 / 1000.0)),
            StatusRow::new("Tool time", format!("{:.1}s", ctx.tool_duration_ms as f64 / 1000.0)),
            StatusRow::new("Lines added", fmt_num(ctx.lines_added)),
            StatusRow::new("Lines removed", fmt_num(ctx.lines_removed)),
        ];
        sections.push(Section { title: "Usage", rows });
    }

    // ── Per-model breakdown ──────────────────────────────────────────────────
    if !ctx.model_usage.is_empty() {
        let rows = ctx.model_usage.iter().map(|(name, inp, out, cr, cw, cost)| {
            StatusRow::colored(
                name.clone(),
                format!(
                    "in={} out={} cache_r={} cache_w={} cost={}",
                    fmt_num(*inp),
                    fmt_num(*out),
                    fmt_num(*cr),
                    fmt_num(*cw),
                    fmt_cost(*cost)
                ),
                Color::DarkGray,
            )
        }).collect();
        sections.push(Section { title: "Per-model usage", rows });
    }

    // ── Modes ────────────────────────────────────────────────────────────────
    {
        fn flag(enabled: bool) -> (&'static str, Color) {
            if enabled { ("enabled", Color::Green) } else { ("disabled", Color::DarkGray) }
        }
        let (vim_str, vim_c) = flag(ctx.vim_mode);
        let (plan_str, plan_c) = flag(ctx.plan_mode);
        let (fast_str, fast_c) = flag(ctx.fast_mode);
        let (brief_str, brief_c) = flag(ctx.brief_mode);

        let rows = vec![
            StatusRow::colored("Vim mode", vim_str, vim_c),
            StatusRow::colored("Plan mode", plan_str, plan_c),
            StatusRow::colored("Fast mode", fast_str, fast_c),
            StatusRow::colored("Brief mode", brief_str, brief_c),
        ];
        sections.push(Section { title: "Modes", rows });
    }

    // ── Configuration ────────────────────────────────────────────────────────
    {
        let mut rows = Vec::new();

        if let Ok(settings_path) = omni_core::config::paths::user_settings_path() {
            let exists = settings_path.exists();
            rows.push(StatusRow::colored(
                "Settings file",
                settings_path.display().to_string(),
                if exists { Color::White } else { Color::DarkGray },
            ));
        }

        // ANTHROPIC_API_KEY presence
        let has_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        rows.push(StatusRow::colored(
            "ANTHROPIC_API_KEY",
            if has_key { "set" } else { "not set" },
            if has_key { Color::Green } else { Color::DarkGray },
        ));

        // ANTHROPIC_BASE_URL override
        if let Ok(base) = std::env::var("ANTHROPIC_BASE_URL") {
            rows.push(StatusRow::colored("ANTHROPIC_BASE_URL", base, Color::Yellow));
        }

        // HTTP_PROXY / HTTPS_PROXY
        for var in &["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            if let Ok(val) = std::env::var(var) {
                rows.push(StatusRow::colored(*var, val, Color::Yellow));
            }
        }

        // NO_COLOR / TERM
        if let Ok(term) = std::env::var("TERM") {
            rows.push(StatusRow::colored("TERM", term, Color::DarkGray));
        }
        if std::env::var("NO_COLOR").is_ok() {
            rows.push(StatusRow::colored("NO_COLOR", "set", Color::Yellow));
        }

        sections.push(Section { title: "Configuration", rows });
    }

    // ── Git ──────────────────────────────────────────────────────────────────
    {
        let mut rows = Vec::new();

        let git_root = find_git_root(&ctx.cwd);
        if let Some(ref root) = git_root {
            rows.push(StatusRow::colored(
                "Git root",
                root.display().to_string(),
                Color::DarkGray,
            ));

            // Branch
            if let Some(branch) = git_current_branch(root) {
                rows.push(StatusRow::new("Branch", branch));
            }

            // Dirty status
            let dirty = git_is_dirty(root);
            rows.push(StatusRow::colored(
                "Dirty",
                if dirty { "yes (uncommitted changes)" } else { "no (clean)" },
                if dirty { Color::Yellow } else { Color::Green },
            ));

            // Recent commit
            if let Some(commit) = git_last_commit(root) {
                rows.push(StatusRow::colored("Last commit", commit, Color::DarkGray));
            }
        } else {
            rows.push(StatusRow::colored("Git", "not a git repository", Color::DarkGray));
        }

        sections.push(Section { title: "Git", rows });
    }

    sections
}

// ── Rendering ────────────────────────────────────────────────────────────────

fn render_sections(sections: Vec<Section>) -> Vec<Line<'static>> {
    let key_width = 22usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    for section in sections {
        // Section header
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", section.title),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "─".repeat(40),
                Style::default().fg(Color::Rgb(60, 60, 80)),
            ),
        ]));

        for row in section.rows {
            let key_padded = format!("  {:<width$}", row.key, width = key_width);
            lines.push(Line::from(vec![
                Span::styled(key_padded, Style::default().fg(Color::Rgb(140, 140, 160))),
                Span::styled(
                    format!(" {}", row.value),
                    Style::default().fg(row.value_color),
                ),
            ]));
        }

        // Blank line between sections
        lines.push(Line::from(""));
    }

    lines
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        return "****".to_string();
    }
    format!("{}...{}", &key[..4], &key[key.len() - 4..])
}

fn fmt_num(n: u64) -> String {
    omni_core::utils::format::format_tokens(n)
}

fn fmt_cost(c: f64) -> String {
    if c < 0.001 {
        format!("${:.4}", c)
    } else if c < 0.01 {
        format!("${:.3}", c)
    } else {
        format!("${:.2}", c)
    }
}

fn find_git_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => return None,
        }
    }
}

fn git_current_branch(root: &std::path::Path) -> Option<String> {
    let head = root.join(".git").join("HEAD");
    let content = std::fs::read_to_string(head).ok()?;
    let content = content.trim();
    if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else {
        // Detached HEAD — show short hash
        Some(content.chars().take(8).collect())
    }
}

fn git_is_dirty(root: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn git_last_commit(root: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=format:%h %s"])
        .current_dir(root)
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    } else {
        None
    }
}
