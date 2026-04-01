use std::path::Path;

use super::system_prompt::{get_knowledge_cutoff, get_marketing_name};

/// Build environment context matching the original `computeSimpleEnvInfo()`.
pub async fn compute_env_info(model_id: &str, project_root: &Path, is_git: bool) -> String {
    let cwd = project_root.display().to_string();
    let platform = std::env::consts::OS;
    let shell = get_shell_info_line();
    let os_version = get_uname_sr();

    // Model description
    let model_description = match get_marketing_name(model_id) {
        Some(name) => format!(
            "You are powered by the model named {}. The exact model ID is {}.",
            name, model_id
        ),
        None => format!("You are powered by the model {}.", model_id),
    };

    // Knowledge cutoff
    let cutoff =
        get_knowledge_cutoff(model_id).map(|c| format!("Assistant knowledge cutoff is {}.", c));

    let mut env_items: Vec<String> = Vec::new();

    env_items.push(format!("Primary working directory: {}", cwd));
    env_items.push(format!(
        "Is a git repository: {}",
        if is_git { "Yes" } else { "No" }
    ));
    env_items.push(format!("Platform: {}", platform));
    env_items.push(shell);
    env_items.push(format!("OS Version: {}", os_version));
    env_items.push(model_description);

    if let Some(cutoff_msg) = cutoff {
        env_items.push(cutoff_msg);
    }

    env_items.push(format!(
        "The most recent Claude model family is Claude 4.5/4.6. Model IDs — Opus 4.6: '{}', \
         Sonnet 4.6: '{}', Haiku 4.5: '{}'. When building AI applications, default to the \
         latest and most capable Claude models.",
        super::system_prompt::OPUS_MODEL_ID,
        super::system_prompt::SONNET_MODEL_ID,
        super::system_prompt::HAIKU_MODEL_ID,
    ));

    env_items.push(
        "Claude Code is available as a CLI in the terminal, desktop app (Mac/Windows), web app \
         (claude.ai/code), and IDE extensions (VS Code, JetBrains)."
            .to_string(),
    );

    env_items.push(format!(
        "Fast mode for Claude Code uses the same {} model with faster output. It does NOT switch \
         to a different model. It can be toggled with /fast.",
        super::system_prompt::FRONTIER_MODEL,
    ));

    let mut out = String::from("# Environment\n");
    out.push_str("You have been invoked in the following environment: \n");
    for item in &env_items {
        out.push_str(&format!(" - {}\n", item));
    }
    out
}

/// Legacy simple environment context (kept for backward compat with callers).
pub fn build_environment_context() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into());

    format!(
        "# Environment\n\
         - Platform: {}\n\
         - Architecture: {}\n\
         - Shell: {}\n\
         - Working directory: {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::var("SHELL").unwrap_or_else(|_| "unknown".into()),
        cwd,
    )
}

/// Returns the shell info line, matching the original's `getShellInfoLine()`.
fn get_shell_info_line() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let shell_name = if shell.contains("zsh") {
        "zsh"
    } else if shell.contains("bash") {
        "bash"
    } else if shell.contains("fish") {
        "fish"
    } else {
        &shell
    };

    if std::env::consts::OS == "windows" {
        format!(
            "Shell: {} (use Unix shell syntax, not Windows — e.g., /dev/null not NUL, forward \
             slashes in paths)",
            shell_name
        )
    } else {
        format!("Shell: {}", shell_name)
    }
}

/// Returns OS type + release, matching the original's `getUnameSR()`.
fn get_uname_sr() -> String {
    // On macOS/Linux, use uname. On failure, fall back to consts.
    if let Ok(output) = std::process::Command::new("uname").args(["-sr"]).output() {
        if output.status.success() {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !result.is_empty() {
                return result;
            }
        }
    }
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}
