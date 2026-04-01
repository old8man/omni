use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Fetch and display comments from a GitHub pull request.
///
/// Matches the TypeScript `pr-comments` command. This is a prompt-type
/// command that instructs the model to use `gh` CLI to fetch PR comments,
/// parse them, and display them in a readable format.
pub struct PrCommentsCommand;

const PR_COMMENTS_ALLOWED_TOOLS: &[&str] = &[
    "Bash(gh pr view:*)",
    "Bash(gh api:*)",
    "Bash(gh pr list:*)",
];

fn get_pr_comments_prompt(args: &str) -> String {
    let user_input = args.trim();
    let additional = if user_input.is_empty() {
        String::new()
    } else {
        format!("\n\nAdditional user input: {}", user_input)
    };

    format!(
        "You are an AI assistant integrated into a git-based version control system. \
         Your task is to fetch and display comments from a GitHub pull request.\n\
         \n\
         Follow these steps:\n\
         \n\
         1. Use `gh pr view --json number,headRepository` to get the PR number and repository info\n\
         2. Use `gh api /repos/{{owner}}/{{repo}}/issues/{{number}}/comments` to get PR-level comments\n\
         3. Use `gh api /repos/{{owner}}/{{repo}}/pulls/{{number}}/comments` to get review comments. \
            Pay particular attention to the following fields: `body`, `diff_hunk`, `path`, `line`, etc. \
            If the comment references some code, consider fetching it using e.g. \
            `gh api /repos/{{owner}}/{{repo}}/contents/{{path}}?ref={{branch}} | jq .content -r | base64 -d`\n\
         4. Parse and format all comments in a readable way\n\
         5. Return ONLY the formatted comments, with no additional text\n\
         \n\
         Format the comments as:\n\
         \n\
         ## Comments\n\
         \n\
         [For each comment thread:]\n\
         - @author file.ts#line:\n\
           ```diff\n\
           [diff_hunk from the API response]\n\
           ```\n\
           > quoted comment text\n\
         \n\
           [any replies indented]\n\
         \n\
         If there are no comments, return \"No comments found.\"\n\
         \n\
         Remember:\n\
         1. Only show the actual comments, no explanatory text\n\
         2. Include both PR-level and code review comments\n\
         3. Preserve the threading/nesting of comment replies\n\
         4. Show the file and line number context for code review comments\n\
         5. Use jq to parse the JSON responses from the GitHub API\
         {}",
        additional
    )
}

#[async_trait]
impl Command for PrCommentsCommand {
    fn name(&self) -> &str {
        "pr-comments"
    }

    fn description(&self) -> &str {
        "Get comments from a GitHub pull request"
    }

    fn usage_hint(&self) -> &str {
        "[pr-number]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Prompt {
            content: get_pr_comments_prompt(args),
            allowed_tools: Some(
                PR_COMMENTS_ALLOWED_TOOLS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            progress_message: Some("fetching PR comments".to_string()),
        }
    }
}
