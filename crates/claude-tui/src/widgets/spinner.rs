//! Spinner widget for indicating background activity.
//!
//! Uses braille spinner frames at 80ms intervals (12.5 fps).
//! Supports multiple concurrent spinners for parallel tool use.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use std::time::Instant;

use crate::theme;

const SPINNER_FRAMES: &[&str] = &[
    "\u{280B}", // ⠋
    "\u{2819}", // ⠙
    "\u{2839}", // ⠹
    "\u{2838}", // ⠸
    "\u{283C}", // ⠼
    "\u{2834}", // ⠴
    "\u{2826}", // ⠦
    "\u{2827}", // ⠧
    "\u{2807}", // ⠇
    "\u{280F}", // ⠏
];

/// Tick interval for the spinner in milliseconds (80ms = 12.5 fps).
pub const SPINNER_TICK_MS: u64 = 80;

#[derive(Clone, Debug)]
pub enum SpinnerVerb {
    Thinking,
    ReadingFile { name: String },
    RunningCommand { name: String },
    Searching,
    Waiting,
    Loading,
    Processing,
    Tool { name: String },
    Stopped,
}

impl SpinnerVerb {
    /// Return the display string for this verb.
    pub fn label(&self) -> String {
        match self {
            SpinnerVerb::Thinking => "Thinking...".to_string(),
            SpinnerVerb::ReadingFile { name } => format!("Reading {}...", name),
            SpinnerVerb::RunningCommand { name } => format!("Running {}...", name),
            SpinnerVerb::Searching => "Searching...".to_string(),
            SpinnerVerb::Waiting => "Waiting...".to_string(),
            SpinnerVerb::Loading => "Loading...".to_string(),
            SpinnerVerb::Processing => "Processing...".to_string(),
            SpinnerVerb::Tool { name } => format!("{}...", name),
            SpinnerVerb::Stopped => "Ready".to_string(),
        }
    }
}

// Keep the old SpinnerMode as a type alias for backward compatibility
pub type SpinnerMode = SpinnerVerb;

/// A single concurrent spinner instance (for parallel tool use).
#[derive(Clone, Debug)]
pub struct SubSpinner {
    pub verb: SpinnerVerb,
    pub start_time: Instant,
    pub frame: usize,
}

impl SubSpinner {
    pub fn new(verb: SpinnerVerb) -> Self {
        Self {
            verb,
            start_time: Instant::now(),
            frame: 0,
        }
    }

    pub fn elapsed_str(&self) -> String {
        let elapsed_ms = self.start_time.elapsed().as_millis() as u64;
        claude_core::utils::format::format_seconds_short(elapsed_ms)
    }

    pub fn advance(&mut self) {
        self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
    }

    pub fn frame_char(&self) -> &'static str {
        SPINNER_FRAMES[self.frame % SPINNER_FRAMES.len()]
    }
}

pub struct SpinnerState {
    pub frame: usize,
    pub mode: SpinnerVerb,
    pub start_time: Instant,
    pub tokens: u64,
    pub active: bool,
    /// Optional tip text to display alongside the spinner.
    pub tip: Option<String>,
    /// Concurrent sub-spinners for parallel tool use.
    pub sub_spinners: Vec<SubSpinner>,
}

impl Default for SpinnerState {
    fn default() -> Self {
        Self::new()
    }
}

impl SpinnerState {
    pub fn new() -> Self {
        Self {
            frame: 0,
            mode: SpinnerVerb::Stopped,
            start_time: Instant::now(),
            tokens: 0,
            active: false,
            tip: None,
            sub_spinners: Vec::new(),
        }
    }

    pub fn start(&mut self, mode: SpinnerVerb) {
        self.mode = mode;
        self.start_time = Instant::now();
        self.tokens = 0;
        self.active = true;
        self.frame = 0;
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.mode = SpinnerVerb::Stopped;
        self.tip = None;
        self.sub_spinners.clear();
    }

    pub fn advance(&mut self) {
        if self.active {
            self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
            for sub in &mut self.sub_spinners {
                sub.advance();
            }
        }
    }

    pub fn elapsed_str(&self) -> String {
        let elapsed_ms = self.start_time.elapsed().as_millis() as u64;
        claude_core::utils::format::format_seconds_short(elapsed_ms)
    }

    /// Add a concurrent sub-spinner for parallel tool use.
    pub fn add_sub_spinner(&mut self, verb: SpinnerVerb) -> usize {
        let idx = self.sub_spinners.len();
        self.sub_spinners.push(SubSpinner::new(verb));
        idx
    }

    /// Remove a sub-spinner by index.
    pub fn remove_sub_spinner(&mut self, idx: usize) {
        if idx < self.sub_spinners.len() {
            self.sub_spinners.remove(idx);
        }
    }

    /// Total height needed to render the spinner (main + sub-spinners).
    pub fn render_height(&self) -> u16 {
        if !self.active {
            return 0;
        }
        1 + self.sub_spinners.len() as u16
    }
}

impl Widget for &SpinnerState {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.active || area.height == 0 {
            return;
        }

        // ── Main spinner line ───────────────────────────────────────────
        let frame_char = SPINNER_FRAMES[self.frame % SPINNER_FRAMES.len()];
        let elapsed = self.elapsed_str();

        let mut spans = vec![
            Span::styled(format!(" {} ", frame_char), theme::STYLE_CYAN),
            Span::styled(self.mode.label(), theme::STYLE_WHITE),
            Span::styled(format!(" ({})", elapsed), theme::STYLE_DARK_GRAY),
        ];

        if self.tokens > 0 {
            spans.push(Span::styled(
                format!(" \u{00B7} {} tokens", self.tokens),
                theme::STYLE_DARK_GRAY,
            ));
        }

        if let Some(ref tip) = self.tip {
            spans.push(Span::styled(
                format!(" \u{00B7} Tip: {}", tip),
                theme::STYLE_DARK_GRAY,
            ));
        }

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);

        // ── Sub-spinners for parallel tool use ──────────────────────────
        for (i, sub) in self.sub_spinners.iter().enumerate() {
            let y = area.y + 1 + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let sub_elapsed = sub.elapsed_str();
            let sub_line = Line::from(vec![
                Span::raw("   "),
                Span::styled(format!("{} ", sub.frame_char()), theme::STYLE_CYAN),
                Span::styled(sub.verb.label(), theme::STYLE_WHITE),
                Span::styled(format!(" ({})", sub_elapsed), theme::STYLE_DARK_GRAY),
            ]);
            buf.set_line(area.x, y, &sub_line, area.width);
        }
    }
}
