use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Git commit workflow — matches the original TypeScript commit command.
///
/// This is a "prompt" type command: it injects a prompt with git context
/// into the conversation for the model to execute. The model then stages
/// files and creates the commit.
pub struct CommitCommand;

const COMMIT_ALLOWED_TOOLS: &[&str] = &[
    "Bash(git add:*)",
    "Bash(git status:*)",
    "Bash(git commit:*)",
];

fn get_commit_prompt() -> String {
    "## Context\n\
     \n\
     - Current git status: !`git status`\n\
     - Current git diff (staged and unstaged changes): !`git diff HEAD`\n\
     - Current branch: !`git branch --show-current`\n\
     - Recent commits: !`git log --oneline -10`\n\
     \n\
     ## Git Safety Protocol\n\
     \n\
     - NEVER update the git config\n\
     - NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless the user explicitly requests it\n\
     - CRITICAL: ALWAYS create NEW commits. NEVER use git commit --amend, unless the user \
     explicitly requests it\n\
     - Do not commit files that likely contain secrets (.env, credentials.json, etc). Warn the \
     user if they specifically request to commit those files\n\
     - If there are no changes to commit (i.e., no untracked files and no modifications), do not \
     create an empty commit\n\
     - Never use git commands with the -i flag (like git rebase -i or git add -i) since they \
     require interactive input which is not supported\n\
     \n\
     ## Your task\n\
     \n\
     Based on the above changes, create a single git commit:\n\
     \n\
     1. Analyze all staged changes and draft a commit message:\n\
        - Look at the recent commits above to follow this repository's commit message style\n\
        - Summarize the nature of the changes (new feature, enhancement, bug fix, refactoring, \
     test, docs, etc.)\n\
        - Ensure the message accurately reflects the changes and their purpose (i.e. \"add\" \
     means a wholly new feature, \"update\" means an enhancement to an existing feature, \"fix\" \
     means a bug fix, etc.)\n\
        - Draft a concise (1-2 sentences) commit message that focuses on the \"why\" rather than \
     the \"what\"\n\
     \n\
     2. Stage relevant files and create the commit using HEREDOC syntax:\n\
     ```\n\
     git commit -m \"$(cat <<'EOF'\n\
     Commit message here.\n\
     EOF\n\
     )\"\n\
     ```\n\
     \n\
     You have the capability to call multiple tools in a single response. Stage and create the \
     commit using a single message. Do not use any other tools or do anything else. Do not send \
     any other text or messages besides these tool calls."
        .to_string()
}

#[async_trait]
impl Command for CommitCommand {
    fn name(&self) -> &str {
        "commit"
    }

    fn description(&self) -> &str {
        "Create a git commit"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Prompt {
            content: get_commit_prompt(),
            allowed_tools: Some(COMMIT_ALLOWED_TOOLS.iter().map(|s| s.to_string()).collect()),
            progress_message: Some("creating commit".to_string()),
        }
    }
}

/// Full commit + push + PR creation workflow.
///
/// Matches the original TypeScript commit-push-pr command.
pub struct CommitPushPrCommand;

const CPR_ALLOWED_TOOLS: &[&str] = &[
    "Bash(git checkout --branch:*)",
    "Bash(git checkout -b:*)",
    "Bash(git add:*)",
    "Bash(git status:*)",
    "Bash(git push:*)",
    "Bash(git commit:*)",
    "Bash(gh pr create:*)",
    "Bash(gh pr edit:*)",
    "Bash(gh pr view:*)",
    "Bash(gh pr merge:*)",
    "ToolSearch",
];

fn get_commit_push_pr_prompt(args: &str) -> String {
    let default_branch = std::process::Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() {
            String::from_utf8(o.stdout).ok().map(|s| {
                s.trim().rsplit('/').next().unwrap_or("main").to_string()
            })
        } else {
            None
        })
        .unwrap_or_else(|| "main".to_string());

    let mut prompt = format!(
        "## Context\n\
         \n\
         - `git status`: !`git status`\n\
         - `git diff HEAD`: !`git diff HEAD`\n\
         - `git branch --show-current`: !`git branch --show-current`\n\
         - `git diff {default_branch}...HEAD`: !`git diff {default_branch}...HEAD`\n\
         - `gh pr view --json number 2>/dev/null || true`: !`gh pr view --json number 2>/dev/null || true`\n\
         \n\
         ## Git Safety Protocol\n\
         \n\
         - NEVER update the git config\n\
         - NEVER run destructive/irreversible git commands (like push --force, hard reset, etc) \
         unless the user explicitly requests them\n\
         - NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless the user explicitly requests it\n\
         - NEVER run force push to main/master, warn the user if they request it\n\
         - Do not commit files that likely contain secrets (.env, credentials.json, etc)\n\
         - Never use git commands with the -i flag (like git rebase -i or git add -i) since they \
         require interactive input which is not supported\n\
         \n\
         ## Your task\n\
         \n\
         Analyze all changes that will be included in the pull request, making sure to look at all \
         relevant commits (NOT just the latest commit, but ALL commits that will be included in the \
         pull request from the git diff {default_branch}...HEAD output above).\n\
         \n\
         Based on the above changes:\n\
         1. Create a new branch if on {default_branch} (use a descriptive name like \
         `username/feature-name`)\n\
         2. Create a single commit with an appropriate message using heredoc syntax:\n\
         ```\n\
         git commit -m \"$(cat <<'EOF'\n\
         Commit message here.\n\
         EOF\n\
         )\"\n\
         ```\n\
         3. Push the branch to origin\n\
         4. If a PR already exists for this branch (check the gh pr view output above), update the \
         PR title and body using `gh pr edit` to reflect the current diff. Otherwise, create a \
         pull request using `gh pr create` with heredoc syntax for the body.\n\
            - IMPORTANT: Keep PR titles short (under 70 characters). Use the body for details.\n\
         ```\n\
         gh pr create --title \"Short, descriptive title\" --body \"$(cat <<'EOF'\n\
         ## Summary\n\
         <1-3 bullet points>\n\
         \n\
         ## Test plan\n\
         [Bulleted markdown checklist of TODOs for testing the pull request...]\n\
         EOF\n\
         )\"\n\
         ```\n\
         \n\
         You have the capability to call multiple tools in a single response. You MUST do all of \
         the above in a single message.\n\
         \n\
         Return the PR URL when you're done, so the user can see it."
    );

    let trimmed = args.trim();
    if !trimmed.is_empty() {
        prompt.push_str(&format!(
            "\n\n## Additional instructions from user\n\n{}",
            trimmed
        ));
    }

    prompt
}

#[async_trait]
impl Command for CommitPushPrCommand {
    fn name(&self) -> &str {
        "commit-push-pr"
    }

    fn aliases(&self) -> &[&str] {
        &["pr"]
    }

    fn description(&self) -> &str {
        "Commit, push, and create a pull request"
    }

    fn usage_hint(&self) -> &str {
        "[description]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Prompt {
            content: get_commit_push_pr_prompt(args),
            allowed_tools: Some(CPR_ALLOWED_TOOLS.iter().map(|s| s.to_string()).collect()),
            progress_message: Some("creating commit and PR".to_string()),
        }
    }
}
