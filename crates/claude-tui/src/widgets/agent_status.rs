//! Agent and teammate status panel widget.
//!
//! Displays the status of background agents and teammates in a sidebar,
//! showing their names, current activities, and completion status.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

/// Status of an individual agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentState {
    /// Agent is idle, waiting for work.
    Idle,
    /// Agent is actively working.
    Running,
    /// Agent has completed its task.
    Completed,
    /// Agent encountered an error.
    Error,
}

impl AgentState {
    /// Return the color for this state.
    fn color(&self) -> Color {
        match self {
            Self::Idle => Color::DarkGray,
            Self::Running => Color::Yellow,
            Self::Completed => Color::Green,
            Self::Error => Color::Red,
        }
    }

    /// Return the icon for this state.
    fn icon(&self) -> &'static str {
        match self {
            Self::Idle => "\u{25CB}",     // ○
            Self::Running => "\u{25CF}",  // ●
            Self::Completed => "\u{2714}", // ✔
            Self::Error => "\u{2718}",     // ✘
        }
    }
}

/// Information about a single agent.
#[derive(Clone, Debug)]
pub struct AgentInfo {
    /// Agent name or identifier.
    pub name: String,
    /// Current task description.
    pub task: String,
    /// Current state.
    pub state: AgentState,
    /// Progress percentage (0-100), if applicable.
    pub progress: Option<u8>,
}

/// State for the agent status panel.
pub struct AgentStatusPanel {
    /// List of tracked agents.
    pub agents: Vec<AgentInfo>,
    /// Whether the panel is visible.
    pub visible: bool,
    /// Scroll offset.
    pub scroll_offset: usize,
}

impl AgentStatusPanel {
    /// Create a new agent status panel.
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            visible: false,
            scroll_offset: 0,
        }
    }

    /// Toggle panel visibility.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Add or update an agent's status.
    pub fn update_agent(&mut self, name: String, task: String, state: AgentState, progress: Option<u8>) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.name == name) {
            agent.task = task;
            agent.state = state;
            agent.progress = progress;
        } else {
            self.agents.push(AgentInfo {
                name,
                task,
                state,
                progress,
            });
        }
    }

    /// Remove an agent from tracking.
    pub fn remove_agent(&mut self, name: &str) {
        self.agents.retain(|a| a.name != name);
    }

    /// Number of running agents.
    pub fn running_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|a| a.state == AgentState::Running)
            .count()
    }

    /// Scroll up.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down.
    pub fn scroll_down(&mut self, n: usize) {
        let max = self.agents.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }
}

impl Default for AgentStatusPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget that renders the agent status panel.
pub struct AgentStatusWidget<'a> {
    panel: &'a AgentStatusPanel,
}

impl<'a> AgentStatusWidget<'a> {
    /// Create a new agent status widget.
    pub fn new(panel: &'a AgentStatusPanel) -> Self {
        Self { panel }
    }

    fn render_content(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if self.panel.agents.is_empty() {
            lines.push(Line::from(Span::styled(
                " No active agents",
                Style::default().fg(Color::DarkGray),
            )));
            return lines;
        }

        let running = self.panel.running_count();
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} agent{} running", running, if running == 1 { "" } else { "s" }),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        for agent in &self.panel.agents {
            let icon = agent.state.icon();
            let color = agent.state.color();

            // Agent name and status icon
            lines.push(Line::from(vec![
                Span::styled(format!(" {} ", icon), Style::default().fg(color)),
                Span::styled(
                    agent.name.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            // Current task
            let task_display = if agent.task.len() > 40 {
                format!("{}...", &agent.task[..37])
            } else {
                agent.task.clone()
            };
            lines.push(Line::from(Span::styled(
                format!("   {}", task_display),
                Style::default().fg(Color::DarkGray),
            )));

            // Progress bar (if applicable)
            if let Some(progress) = agent.progress {
                let bar_width = 15;
                let filled = (progress as usize * bar_width) / 100;
                let empty = bar_width - filled;
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        "\u{2588}".repeat(filled),
                        Style::default().fg(color),
                    ),
                    Span::styled(
                        "\u{2591}".repeat(empty),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!(" {}%", progress),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            lines.push(Line::from(""));
        }

        lines
    }
}

impl<'a> Widget for AgentStatusWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.panel.visible || area.height < 3 || area.width < 10 {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Agents ")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        let content = self.render_content();
        let visible_height = inner.height as usize;
        let scroll = self
            .panel
            .scroll_offset
            .min(content.len().saturating_sub(visible_height));
        let end = (scroll + visible_height).min(content.len());

        for (i, line) in content[scroll..end].iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x, y, line, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_agent() {
        let mut panel = AgentStatusPanel::new();
        panel.update_agent(
            "worker-1".to_string(),
            "Reading files".to_string(),
            AgentState::Running,
            Some(50),
        );
        assert_eq!(panel.agents.len(), 1);
        assert_eq!(panel.running_count(), 1);

        // Update existing
        panel.update_agent(
            "worker-1".to_string(),
            "Done".to_string(),
            AgentState::Completed,
            None,
        );
        assert_eq!(panel.agents.len(), 1);
        assert_eq!(panel.running_count(), 0);
    }

    #[test]
    fn test_remove_agent() {
        let mut panel = AgentStatusPanel::new();
        panel.update_agent(
            "worker-1".to_string(),
            "task".to_string(),
            AgentState::Idle,
            None,
        );
        panel.remove_agent("worker-1");
        assert!(panel.agents.is_empty());
    }

    #[test]
    fn test_toggle() {
        let mut panel = AgentStatusPanel::new();
        assert!(!panel.visible);
        panel.toggle();
        assert!(panel.visible);
    }

    #[test]
    fn test_agent_state_display() {
        assert_eq!(AgentState::Running.color(), Color::Yellow);
        assert_eq!(AgentState::Completed.icon(), "\u{2714}");
    }
}
