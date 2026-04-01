use claude_tools::bash::BashTool;
use claude_tools::registry::{ToolExecutor, ToolUseContext};
use serde_json::json;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

fn make_ctx(dir: PathBuf) -> ToolUseContext {
    ToolUseContext::with_working_directory(dir)
}

fn tmpdir() -> PathBuf {
    std::env::temp_dir()
}

#[tokio::test]
async fn test_bash_echo() {
    let tool = BashTool;
    let ctx = make_ctx(tmpdir());
    let cancel = CancellationToken::new();
    let input = json!({ "command": "echo hello" });
    let result = tool.call(&input, &ctx, cancel, None).await.unwrap();
    assert!(!result.is_error);
    let stdout = result.data["stdout"].as_str().unwrap();
    let code = result.data["code"].as_i64().unwrap();
    assert_eq!(stdout, "hello\n");
    assert_eq!(code, 0);
}

#[tokio::test]
async fn test_bash_exit_code() {
    let tool = BashTool;
    let ctx = make_ctx(tmpdir());
    let cancel = CancellationToken::new();
    let input = json!({ "command": "exit 42" });
    let result = tool.call(&input, &ctx, cancel, None).await.unwrap();
    assert!(!result.is_error);
    let code = result.data["code"].as_i64().unwrap();
    assert_eq!(code, 42);
}

#[tokio::test]
async fn test_bash_stderr() {
    let tool = BashTool;
    let ctx = make_ctx(tmpdir());
    let cancel = CancellationToken::new();
    // Use a command that naturally produces stderr without redirection operators
    // (which may be blocked by security validation).
    let input = json!({ "command": "ls /nonexistent_path_12345 2>&1 || true" });
    let result = tool.call(&input, &ctx, cancel, None).await.unwrap();
    assert!(!result.is_error);
    let stdout = result.data["stdout"].as_str().unwrap_or("");
    assert!(
        stdout.contains("No such file") || stdout.contains("cannot access") || stdout.contains("not found"),
        "output should contain error message about nonexistent path, got: {:?}",
        stdout
    );
}

#[tokio::test]
async fn test_bash_cwd() {
    let tool = BashTool;
    let working_dir = tmpdir();
    let ctx = make_ctx(working_dir.clone());
    let cancel = CancellationToken::new();
    let input = json!({ "command": "pwd" });
    let result = tool.call(&input, &ctx, cancel, None).await.unwrap();
    assert!(!result.is_error);
    let stdout = result.data["stdout"].as_str().unwrap().trim().to_string();
    // Canonicalize both sides to handle macOS /private/tmp symlinks
    let actual = std::fs::canonicalize(&stdout).unwrap_or_else(|_| PathBuf::from(&stdout));
    let expected = std::fs::canonicalize(&working_dir).unwrap_or(working_dir);
    assert_eq!(
        actual, expected,
        "pwd output should match working_directory"
    );
}

#[tokio::test]
async fn test_bash_cancellation() {
    let tool = BashTool;
    let ctx = make_ctx(tmpdir());
    let cancel = CancellationToken::new();
    // Cancel before running
    cancel.cancel();
    let input = json!({ "command": "sleep 10" });
    let result = tool.call(&input, &ctx, cancel, None).await.unwrap();
    assert!(!result.is_error);
    let interrupted = result.data["interrupted"].as_bool().unwrap();
    assert!(
        interrupted,
        "should be interrupted when cancel token is already cancelled"
    );
}
