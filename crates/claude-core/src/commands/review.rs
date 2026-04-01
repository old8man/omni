use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};

/// Reviews a pull request — matches the original TypeScript review command.
///
/// This is a "prompt" type command that injects a code review prompt for
/// the model to execute using gh CLI tools.
pub struct ReviewCommand;

fn get_review_prompt(args: &str) -> String {
    format!(
        "You are an expert code reviewer. Follow these steps:\n\
         \n\
         1. If no PR number is provided in the args, run `gh pr list` to show open PRs\n\
         2. If a PR number is provided, run `gh pr view <number>` to get PR details\n\
         3. Run `gh pr diff <number>` to get the diff\n\
         4. Analyze the changes and provide a thorough code review that includes:\n\
            - Overview of what the PR does\n\
            - Analysis of code quality and style\n\
            - Specific suggestions for improvements\n\
            - Any potential issues or risks\n\
         \n\
         Keep your review concise but thorough. Focus on:\n\
         - Code correctness\n\
         - Following project conventions\n\
         - Performance implications\n\
         - Test coverage\n\
         - Security considerations\n\
         \n\
         Format your review with clear sections and bullet points.\n\
         \n\
         PR number: {}",
        args.trim()
    )
}

#[async_trait]
impl Command for ReviewCommand {
    fn name(&self) -> &str {
        "review"
    }

    fn description(&self) -> &str {
        "Review a pull request"
    }

    fn usage_hint(&self) -> &str {
        "[pr-number]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Prompt {
            content: get_review_prompt(args),
            allowed_tools: None, // All tools available
            progress_message: Some("reviewing pull request".to_string()),
        }
    }
}

/// Security-focused review of branch changes.
///
/// Matches the original TypeScript security-review command with its full
/// vulnerability analysis methodology and false-positive filtering.
pub struct SecurityReviewCommand;

const SECURITY_REVIEW_ALLOWED_TOOLS: &[&str] = &[
    "Bash(git diff:*)",
    "Bash(git status:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git remote show:*)",
    "Read",
    "Glob",
    "Grep",
    "Agent",
];

fn get_security_review_prompt() -> String {
    "You are a senior security engineer conducting a focused security review of the changes on \
     this branch.\n\
     \n\
     GIT STATUS:\n\
     \n\
     ```\n\
     !`git status`\n\
     ```\n\
     \n\
     FILES MODIFIED:\n\
     \n\
     ```\n\
     !`git diff --name-only origin/HEAD...`\n\
     ```\n\
     \n\
     COMMITS:\n\
     \n\
     ```\n\
     !`git log --no-decorate origin/HEAD...`\n\
     ```\n\
     \n\
     DIFF CONTENT:\n\
     \n\
     ```\n\
     !`git diff origin/HEAD...`\n\
     ```\n\
     \n\
     Review the complete diff above. This contains all code changes in the PR.\n\
     \n\
     \n\
     OBJECTIVE:\n\
     Perform a security-focused code review to identify HIGH-CONFIDENCE security vulnerabilities \
     that could have real exploitation potential. This is not a general code review - focus ONLY \
     on security implications newly added by this PR. Do not comment on existing security \
     concerns.\n\
     \n\
     CRITICAL INSTRUCTIONS:\n\
     1. MINIMIZE FALSE POSITIVES: Only flag issues where you're >80% confident of actual \
     exploitability\n\
     2. AVOID NOISE: Skip theoretical issues, style concerns, or low-impact findings\n\
     3. FOCUS ON IMPACT: Prioritize vulnerabilities that could lead to unauthorized access, data \
     breaches, or system compromise\n\
     4. EXCLUSIONS: Do NOT report the following issue types:\n\
        - Denial of Service (DOS) vulnerabilities, even if they allow service disruption\n\
        - Secrets or sensitive data stored on disk (these are handled by other processes)\n\
        - Rate limiting or resource exhaustion issues\n\
     \n\
     SECURITY CATEGORIES TO EXAMINE:\n\
     \n\
     **Input Validation Vulnerabilities:**\n\
     - SQL injection via unsanitized user input\n\
     - Command injection in system calls or subprocesses\n\
     - XXE injection in XML parsing\n\
     - Template injection in templating engines\n\
     - NoSQL injection in database queries\n\
     - Path traversal in file operations\n\
     \n\
     **Authentication & Authorization Issues:**\n\
     - Authentication bypass logic\n\
     - Privilege escalation paths\n\
     - Session management flaws\n\
     - JWT token vulnerabilities\n\
     - Authorization logic bypasses\n\
     \n\
     **Crypto & Secrets Management:**\n\
     - Hardcoded API keys, passwords, or tokens\n\
     - Weak cryptographic algorithms or implementations\n\
     - Improper key storage or management\n\
     - Cryptographic randomness issues\n\
     - Certificate validation bypasses\n\
     \n\
     **Injection & Code Execution:**\n\
     - Remote code execution via deserialization\n\
     - Pickle injection in Python\n\
     - YAML deserialization vulnerabilities\n\
     - Eval injection in dynamic code execution\n\
     - XSS vulnerabilities in web applications (reflected, stored, DOM-based)\n\
     \n\
     **Data Exposure:**\n\
     - Sensitive data logging or storage\n\
     - PII handling violations\n\
     - API endpoint data leakage\n\
     - Debug information exposure\n\
     \n\
     Additional notes:\n\
     - Even if something is only exploitable from the local network, it can still be a HIGH \
     severity issue\n\
     \n\
     ANALYSIS METHODOLOGY:\n\
     \n\
     Phase 1 - Repository Context Research (Use file search tools):\n\
     - Identify existing security frameworks and libraries in use\n\
     - Look for established secure coding patterns in the codebase\n\
     - Examine existing sanitization and validation patterns\n\
     - Understand the project's security model and threat model\n\
     \n\
     Phase 2 - Comparative Analysis:\n\
     - Compare new code changes against existing security patterns\n\
     - Identify deviations from established secure practices\n\
     - Look for inconsistent security implementations\n\
     - Flag code that introduces new attack surfaces\n\
     \n\
     Phase 3 - Vulnerability Assessment:\n\
     - Examine each modified file for security implications\n\
     - Trace data flow from user inputs to sensitive operations\n\
     - Look for privilege boundaries being crossed unsafely\n\
     - Identify injection points and unsafe deserialization\n\
     \n\
     REQUIRED OUTPUT FORMAT:\n\
     \n\
     You MUST output your findings in markdown. The markdown output should contain the file, \
     line number, severity, category (e.g. `sql_injection` or `xss`), description, exploit \
     scenario, and fix recommendation.\n\
     \n\
     For example:\n\
     \n\
     # Vuln 1: XSS: `foo.py:42`\n\
     \n\
     * Severity: High\n\
     * Description: User input from `username` parameter is directly interpolated into HTML \
     without escaping, allowing reflected XSS attacks\n\
     * Exploit Scenario: Attacker crafts URL like /bar?q=<script>alert(document.cookie)</script> \
     to execute JavaScript in victim's browser, enabling session hijacking or data theft\n\
     * Recommendation: Use Flask's escape() function or Jinja2 templates with auto-escaping \
     enabled for all user inputs rendered in HTML\n\
     \n\
     SEVERITY GUIDELINES:\n\
     - **HIGH**: Directly exploitable vulnerabilities leading to RCE, data breach, or \
     authentication bypass\n\
     - **MEDIUM**: Vulnerabilities requiring specific conditions but with significant impact\n\
     - **LOW**: Defense-in-depth issues or lower-impact vulnerabilities\n\
     \n\
     CONFIDENCE SCORING:\n\
     - 0.9-1.0: Certain exploit path identified, tested if possible\n\
     - 0.8-0.9: Clear vulnerability pattern with known exploitation methods\n\
     - 0.7-0.8: Suspicious pattern requiring specific conditions to exploit\n\
     - Below 0.7: Don't report (too speculative)\n\
     \n\
     FINAL REMINDER:\n\
     Focus on HIGH and MEDIUM findings only. Better to miss some theoretical issues than flood \
     the report with false positives. Each finding should be something a security engineer would \
     confidently raise in a PR review.\n\
     \n\
     START ANALYSIS:\n\
     \n\
     Begin your analysis now. Do this in 3 steps:\n\
     \n\
     1. Use a sub-task to identify vulnerabilities. Use the repository exploration tools to \
     understand the codebase context, then analyze the PR changes for security implications.\n\
     2. Then for each vulnerability identified by the above sub-task, create a new sub-task to \
     filter out false-positives. Launch these sub-tasks as parallel sub-tasks.\n\
     3. Filter out any vulnerabilities where the sub-task reported a confidence less than 8.\n\
     \n\
     Your final reply must contain the markdown report and nothing else."
        .to_string()
}

#[async_trait]
impl Command for SecurityReviewCommand {
    fn name(&self) -> &str {
        "security-review"
    }

    fn description(&self) -> &str {
        "Complete a security review of the pending changes on the current branch"
    }

    async fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Prompt {
            content: get_security_review_prompt(),
            allowed_tools: Some(
                SECURITY_REVIEW_ALLOWED_TOOLS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            progress_message: Some("analyzing code changes for security risks".to_string()),
        }
    }
}
