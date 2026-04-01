use claude_core::context::environment::*;
use claude_core::context::system_prompt::*;

#[test]
fn test_environment_context_contains_platform() {
    let ctx = build_environment_context();
    assert!(ctx.contains("Platform:"));
    assert!(ctx.contains(std::env::consts::OS));
}

#[test]
fn test_environment_context_contains_cwd() {
    let ctx = build_environment_context();
    assert!(ctx.contains("Working directory:"));
}

#[tokio::test]
async fn test_build_system_prompt_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = vec![
        ("Read".into(), "Read files".into()),
        ("Bash".into(), "Run commands".into()),
    ];
    let blocks = build_system_prompt(tmp.path(), &tools).await.unwrap();
    // Should have multiple sections: attribution, intro, system, doing tasks, actions, tools, tone, etc.
    assert!(blocks.len() >= 5);

    // The prompt should contain software engineering guidance somewhere
    let all_text: String = blocks
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all_text.contains("software engineering"));
}

#[tokio::test]
async fn test_build_system_prompt_includes_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = vec![("Grep".into(), "Search files".into())];
    let blocks = build_system_prompt(tmp.path(), &tools).await.unwrap();

    let all_text: String = blocks
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // The tools section references Grep in its guidance
    assert!(all_text.contains("Grep"));
}

#[tokio::test]
async fn test_build_system_prompt_full_with_model() {
    let tmp = tempfile::tempdir().unwrap();
    let tool_names = vec!["Read".to_string(), "Bash".to_string(), "Agent".to_string()];
    let blocks =
        build_system_prompt_full(tmp.path(), "claude-opus-4-6", &tool_names, None, None, None, None)
            .await
            .unwrap();

    let all_text: String = blocks
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // Should contain the model name in the environment section
    assert!(all_text.contains("Opus 4.6"));
    // Should contain the dynamic boundary marker
    assert!(all_text.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
    // Should contain agent guidance since Agent is in enabled tools
    assert!(all_text.contains("Agent"));
}

#[tokio::test]
async fn test_build_system_prompt_with_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let tool_names = vec!["Read".to_string()];
    let blocks = build_system_prompt_full(
        tmp.path(),
        "claude-sonnet-4-6",
        &tool_names,
        Some("# Memory\nRemember that the user prefers Rust."),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let all_text: String = blocks
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(all_text.contains("Remember that the user prefers Rust"));
}

#[tokio::test]
async fn test_build_system_prompt_with_language() {
    let tmp = tempfile::tempdir().unwrap();
    let tool_names = vec!["Read".to_string()];
    let blocks = build_system_prompt_full(
        tmp.path(),
        "claude-sonnet-4-6",
        &tool_names,
        None,
        None,
        Some("Japanese"),
        None,
    )
    .await
    .unwrap();

    let all_text: String = blocks
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(all_text.contains("Japanese"));
}

#[test]
fn test_knowledge_cutoff() {
    assert_eq!(get_knowledge_cutoff("claude-opus-4-6"), Some("May 2025"));
    assert_eq!(
        get_knowledge_cutoff("claude-sonnet-4-6"),
        Some("August 2025")
    );
    assert_eq!(
        get_knowledge_cutoff("claude-haiku-4-5-20251001"),
        Some("February 2025")
    );
    assert_eq!(get_knowledge_cutoff("some-random-model"), None);
}

#[test]
fn test_marketing_name() {
    assert_eq!(
        get_marketing_name("claude-opus-4-6"),
        Some("Opus 4.6".to_string())
    );
    assert_eq!(
        get_marketing_name("claude-sonnet-4-6"),
        Some("Sonnet 4.6".to_string())
    );
    // 1M context suffix
    assert_eq!(
        get_marketing_name("claude-opus-4-6[1m]"),
        Some("Opus 4.6 (with 1M context)".to_string())
    );
    // Claude 3.x format (with "Claude" prefix)
    assert_eq!(
        get_marketing_name("claude-3-7-sonnet-20250219"),
        Some("Claude 3.7 Sonnet".to_string())
    );
    assert_eq!(get_marketing_name("unknown-model"), None);
}

#[tokio::test]
async fn test_git_context_in_non_git_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let git_ctx = claude_core::context::git::get_git_context(tmp.path())
        .await
        .unwrap();
    assert!(git_ctx.is_none());
}
