/// Information about a single tool invocation for summary generation.
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
}

/// System prompt used when generating tool-use summaries via the model.
pub const TOOL_USE_SUMMARY_SYSTEM_PROMPT: &str = r#"Write a short summary label describing what these tool calls accomplished. It appears as a single-line row in a mobile app and truncates around 30 characters, so think git-commit-subject, not sentence.

Keep the verb in past tense and the most distinctive noun. Drop articles, connectors, and long location context first.

Examples:
- Searched in auth/
- Fixed NPE in UserService
- Created signup endpoint
- Read config.json
- Ran failing tests"#;

/// Generate a human-readable summary of completed tool calls.
///
/// This builds a prompt suitable for sending to a small/fast model (e.g. Haiku)
/// and returns the rendered prompt text. The caller is responsible for actually
/// querying the model, since the API layer is outside this module's scope.
///
/// Returns `None` if there are no tools to summarize.
pub fn build_summary_prompt(
    tools: &[ToolInfo],
    last_assistant_text: Option<&str>,
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let tool_summaries: Vec<String> = tools
        .iter()
        .map(|tool| {
            let input_str = truncate_json(&tool.input, 300);
            let output_str = truncate_json(&tool.output, 300);
            format!("Tool: {}\nInput: {}\nOutput: {}", tool.name, input_str, output_str)
        })
        .collect();

    let context_prefix = match last_assistant_text {
        Some(text) => {
            let truncated = if text.len() > 200 { &text[..200] } else { text };
            format!(
                "User's intent (from assistant's last message): {}\n\n",
                truncated
            )
        }
        None => String::new(),
    };

    Some(format!(
        "{}Tools completed:\n\n{}\n\nLabel:",
        context_prefix,
        tool_summaries.join("\n\n")
    ))
}

/// Generate a simple summary without calling the model, based on tool names.
///
/// This is a lightweight fallback when model-based summarization is unavailable.
pub fn generate_simple_summary(tools: &[ToolInfo]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    if tools.len() == 1 {
        let tool = &tools[0];
        let detail = extract_detail(&tool.name, &tool.input);
        return Some(match detail {
            Some(d) => format!("{} {}", past_tense_verb(&tool.name), d),
            None => format!("Used {}", tool.name),
        });
    }

    // Multiple tools — group by name
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == tool.name.as_str()) {
            entry.1 += 1;
        } else {
            counts.push((&tool.name, 1));
        }
    }

    let parts: Vec<String> = counts
        .iter()
        .map(|(name, count)| {
            if *count == 1 {
                name.to_string()
            } else {
                format!("{} x{}", name, count)
            }
        })
        .collect();

    Some(format!("Used {}", parts.join(", ")))
}

/// Extract a meaningful detail from the tool input for the summary.
fn extract_detail(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Read" | "Edit" | "Write" | "NotebookEdit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| {
                // Use just the filename
                std::path::Path::new(p)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            }),
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|c| {
                let truncated = if c.len() > 30 {
                    format!("{}...", &c[..27])
                } else {
                    c.to_string()
                };
                truncated
            }),
        "Glob" | "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| {
                if p.len() > 30 {
                    format!("{}...", &p[..27])
                } else {
                    p.to_string()
                }
            }),
        _ => None,
    }
}

/// Map tool names to past-tense verbs.
fn past_tense_verb(tool_name: &str) -> &str {
    match tool_name {
        "Read" => "Read",
        "Edit" => "Edited",
        "Write" => "Created",
        "Bash" => "Ran",
        "Glob" => "Searched",
        "Grep" => "Searched",
        "NotebookEdit" => "Edited",
        _ => "Used",
    }
}

/// Truncate a JSON value to a maximum string length.
fn truncate_json(value: &serde_json::Value, max_length: usize) -> String {
    match serde_json::to_string(value) {
        Ok(s) => {
            if s.len() <= max_length {
                s
            } else {
                format!("{}...", &s[..max_length.saturating_sub(3)])
            }
        }
        Err(_) => "[unable to serialize]".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_summary_single_tool() {
        let tools = vec![ToolInfo {
            name: "Read".into(),
            input: json!({"file_path": "/home/user/src/main.rs"}),
            output: json!("file content"),
        }];
        let summary = generate_simple_summary(&tools).unwrap();
        assert_eq!(summary, "Read main.rs");
    }

    #[test]
    fn test_simple_summary_multiple_tools() {
        let tools = vec![
            ToolInfo {
                name: "Read".into(),
                input: json!({"file_path": "a.rs"}),
                output: json!(""),
            },
            ToolInfo {
                name: "Read".into(),
                input: json!({"file_path": "b.rs"}),
                output: json!(""),
            },
            ToolInfo {
                name: "Bash".into(),
                input: json!({"command": "cargo test"}),
                output: json!("ok"),
            },
        ];
        let summary = generate_simple_summary(&tools).unwrap();
        assert_eq!(summary, "Used Read x2, Bash");
    }

    #[test]
    fn test_build_summary_prompt_empty() {
        assert!(build_summary_prompt(&[], None).is_none());
    }

    #[test]
    fn test_build_summary_prompt_with_tools() {
        let tools = vec![ToolInfo {
            name: "Bash".into(),
            input: json!({"command": "ls"}),
            output: json!("files"),
        }];
        let prompt = build_summary_prompt(&tools, Some("listing files")).unwrap();
        assert!(prompt.contains("Tools completed:"));
        assert!(prompt.contains("listing files"));
    }
}
