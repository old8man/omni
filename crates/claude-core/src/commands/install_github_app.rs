use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Set up Claude GitHub Actions for a repository.
///
/// Matches the TypeScript `install-github-app` command. Walks the user
/// through configuring a GitHub Actions workflow that lets Claude review
/// PRs and respond to `@claude` mentions.
pub struct InstallGithubAppCommand;

const GITHUB_ACTION_SETUP_DOCS_URL: &str =
    "https://docs.anthropic.com/en/docs/claude-code/github-actions";

#[async_trait]
impl Command for InstallGithubAppCommand {
    fn name(&self) -> &str {
        "install-github-app"
    }

    fn description(&self) -> &str {
        "Set up Claude GitHub Actions for a repository"
    }

    async fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        // Check if command is disabled
        if std::env::var("DISABLE_INSTALL_GITHUB_APP_COMMAND")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            return CommandResult::Output(
                "The install-github-app command is currently disabled.".to_string(),
            );
        }

        // Detect current repo using gh CLI
        let repo_info = detect_github_repo(ctx);

        let mut lines = vec!["Claude GitHub Actions Setup".to_string()];
        lines.push("==========================".to_string());
        lines.push(String::new());

        match repo_info {
            Some(repo) => {
                lines.push(format!("Detected repository: {}", repo));
                lines.push(String::new());
                lines.push("To set up Claude GitHub Actions:".to_string());
                lines.push(String::new());
                lines.push("1. Install the Claude GitHub App on your repository".to_string());
                lines.push(format!(
                    "   Visit: https://github.com/apps/claude/installations/new"
                ));
                lines.push(String::new());
                lines.push("2. Add the ANTHROPIC_API_KEY secret to your repository".to_string());
                lines.push(format!(
                    "   Visit: https://github.com/{}/settings/secrets/actions",
                    repo
                ));
                lines.push(String::new());
                lines.push(
                    "3. Create a workflow file at .github/workflows/claude.yml".to_string(),
                );
                lines.push(String::new());
                lines.push(format!(
                    "For detailed instructions, see: {}",
                    GITHUB_ACTION_SETUP_DOCS_URL
                ));
            }
            None => {
                lines.push(
                    "Could not detect a GitHub repository in the current directory.".to_string(),
                );
                lines.push(String::new());
                lines.push("Please navigate to a git repository and try again, or visit:".to_string());
                lines.push(format!("  {}", GITHUB_ACTION_SETUP_DOCS_URL));
            }
        }

        CommandResult::Output(lines.join("\n"))
    }
}

/// Detect the current GitHub repository using `gh` CLI or git remote.
fn detect_github_repo(ctx: &CommandContext) -> Option<String> {
    // Try gh CLI first
    let gh_result = std::process::Command::new("gh")
        .args(["repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"])
        .current_dir(&ctx.cwd)
        .output();

    if let Ok(output) = gh_result {
        if output.status.success() {
            let repo = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !repo.is_empty() {
                return Some(repo);
            }
        }
    }

    // Fall back to parsing git remote
    let git_result = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&ctx.cwd)
        .output();

    if let Ok(output) = git_result {
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return parse_github_repo_from_url(&url);
        }
    }

    None
}

/// Parse owner/repo from a GitHub URL (HTTPS or SSH).
fn parse_github_repo_from_url(url: &str) -> Option<String> {
    // Handle SSH: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let repo = rest.trim_end_matches(".git");
        return Some(repo.to_string());
    }

    // Handle HTTPS: https://github.com/owner/repo.git
    if url.contains("github.com/") {
        let parts: Vec<&str> = url.split("github.com/").collect();
        if parts.len() >= 2 {
            let repo = parts[1].trim_end_matches(".git");
            return Some(repo.to_string());
        }
    }

    None
}
