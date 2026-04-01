use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use std::time::Instant;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Debug)]
pub enum SpinnerMode {
    Thinking,
    Waiting,
    Loading,
    Processing,
    Tool { name: String },
    Stopped,
}

impl SpinnerMode {
    pub fn label(&self) -> &str {
        match self {
            SpinnerMode::Thinking => "Thinking",
            SpinnerMode::Waiting => "Waiting",
            SpinnerMode::Loading => "Loading",
            SpinnerMode::Processing => "Processing",
            SpinnerMode::Tool { name } => name,
            SpinnerMode::Stopped => "Ready",
        }
    }
}

pub struct SpinnerState {
    pub frame: usize,
    pub mode: SpinnerMode,
    pub start_time: Instant,
    pub tokens: u64,
    pub active: bool,
    /// Optional tip text to display alongside the spinner.
    pub tip: Option<String>,
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
            mode: SpinnerMode::Stopped,
            start_time: Instant::now(),
            tokens: 0,
            active: false,
            tip: None,
        }
    }

    pub fn start(&mut self, mode: SpinnerMode) {
        self.mode = mode;
        self.start_time = Instant::now();
        self.tokens = 0;
        self.active = true;
        self.frame = 0;
        // tip is set externally by the caller after start() if desired
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.mode = SpinnerMode::Stopped;
        self.tip = None;
    }

    pub fn advance(&mut self) {
        if self.active {
            self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
        }
    }

    pub fn elapsed_str(&self) -> String {
        let elapsed_ms = self.start_time.elapsed().as_millis() as u64;
        claude_core::utils::format::format_seconds_short(elapsed_ms)
    }
}

impl Widget for &SpinnerState {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.active || area.height == 0 {
            return;
        }
        let frame_char = SPINNER_FRAMES[self.frame % SPINNER_FRAMES.len()];
        let elapsed = self.elapsed_str();

        let mut spans = vec![
            Span::styled(format!("{} ", frame_char), Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} ", self.mode.label())),
            Span::styled(
                format!("({})", elapsed),
                Style::default().fg(Color::DarkGray),
            ),
        ];

        if self.tokens > 0 {
            spans.push(Span::styled(
                format!(" · {} tokens", self.tokens),
                Style::default().fg(Color::DarkGray),
            ));
        }

        if let Some(ref tip) = self.tip {
            spans.push(Span::styled(
                format!(" · Tip: {}", tip),
                Style::default().fg(Color::DarkGray),
            ));
        }

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}
