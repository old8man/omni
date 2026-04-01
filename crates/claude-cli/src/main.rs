use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "claude-rs",
    about = "Claude Code - AI coding assistant (Rust port)",
    version
)]
pub struct Cli {
    /// Initial prompt (non-interactive mode)
    pub prompt: Option<String>,

    /// Model to use
    #[arg(short, long)]
    pub model: Option<String>,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Skip all permission checks (dangerous)
    #[arg(long)]
    pub dangerously_skip_permissions: bool,

    /// Working directory
    #[arg(short = 'C', long = "cd")]
    pub working_dir: Option<PathBuf>,

    /// Resume session by ID
    #[arg(long)]
    pub resume: Option<String>,

    /// Max conversation turns (non-interactive)
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Append text to system prompt
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// Enable KAIROS always-on assistant mode
    #[arg(long)]
    pub assistant: bool,

    /// Enable brief-only output mode (SendUserMessage for all output)
    #[arg(long)]
    pub brief: bool,

    /// Token budget for task-budgets beta (e.g. 500000)
    #[arg(long)]
    pub task_budget: Option<u64>,

    /// Prompt caching breakpoint TTL in seconds (0 = disabled)
    #[arg(long, default_value = "300")]
    pub cache_ttl: u64,

    #[command(subcommand)]
    pub command: Option<SubCommand>,
}

#[derive(clap::Subcommand)]
pub enum SubCommand {
    /// Authenticate with Anthropic
    Login,
    /// Remove stored credentials
    Logout,
    /// Show current configuration
    Config,
    /// Start Remote Control bridge mode
    #[command(name = "remote-control")]
    RemoteControl {
        /// Maximum concurrent sessions
        #[arg(long, default_value = "1")]
        max_sessions: u32,
        /// Spawn mode: single-session, worktree, or same-dir
        #[arg(long, default_value = "single-session")]
        spawn_mode: String,
        /// Resume an existing session by ID
        #[arg(long)]
        session_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle subcommands that don't need full bootstrap
    match &cli.command {
        Some(SubCommand::Login) => {
            eprintln!("\x1b[1mClaude Code — Login\x1b[0m");

            // Check if already logged in
            if let Some(existing) = claude_core::auth::storage::load_tokens().await? {
                if !claude_core::auth::pkce::is_token_expired(existing.expires_at) {
                    eprintln!();
                    eprintln!("  You are already logged in.");
                    eprintln!("  Run \x1b[1mclaude logout\x1b[0m first to switch accounts.");
                    return Ok(());
                }
            }

            // Run the full PKCE OAuth flow (opens browser, waits for callback)
            let result = claude_core::auth::pkce::run_oauth_login(true).await?;

            // Store tokens securely
            claude_core::auth::storage::store_tokens(&result.tokens).await?;

            eprintln!("\x1b[32m  Login successful!\x1b[0m");
            eprintln!();
            if claude_core::auth::oauth_config::has_inference_scope(&result.tokens.scopes) {
                eprintln!("  Authenticated with Claude.ai (subscriber).");
            } else {
                eprintln!("  Authenticated with Anthropic Console.");
            }
            eprintln!("  Credentials stored securely.");
            return Ok(());
        }
        Some(SubCommand::Logout) => {
            eprintln!("\x1b[1mClaude Code — Logout\x1b[0m");

            // Delete stored credentials (file + keychain)
            claude_core::auth::storage::delete_tokens().await?;

            eprintln!();
            eprintln!("  \x1b[32mLogged out successfully.\x1b[0m");
            eprintln!("  Stored credentials have been removed.");
            return Ok(());
        }
        Some(SubCommand::RemoteControl {
            max_sessions,
            spawn_mode,
            session_id,
        }) => {
            #[cfg(feature = "bridge")]
            {
                use claude_core::bridge::types::{BridgeConfig, SpawnMode};

                let cwd = cli
                    .working_dir
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(std::env::current_dir)?;
                let mode = match spawn_mode.as_str() {
                    "worktree" => SpawnMode::Worktree,
                    "same-dir" => SpawnMode::SameDir,
                    _ => SpawnMode::SingleSession,
                };

                // Detect git info
                let branch = std::process::Command::new("git")
                    .args(["rev-parse", "--abbrev-ref", "HEAD"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();

                let git_repo_url = std::process::Command::new("git")
                    .args(["remote", "get-url", "origin"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

                let hostname = hostname::get()
                    .map(|h| h.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "unknown".to_string());

                let config = BridgeConfig {
                    dir: cwd.to_string_lossy().to_string(),
                    machine_name: hostname,
                    branch,
                    git_repo_url,
                    max_sessions: *max_sessions,
                    spawn_mode: mode,
                    verbose: cli.verbose,
                    sandbox: false,
                    bridge_id: uuid::Uuid::new_v4().to_string(),
                    worker_type: "claude_code".to_string(),
                    environment_id: uuid::Uuid::new_v4().to_string(),
                    reuse_environment_id: None,
                    api_base_url: "https://api.anthropic.com".to_string(),
                    session_ingress_url: "https://api.anthropic.com".to_string(),
                    debug_file: None,
                    session_timeout_ms: None,
                };

                eprintln!("Remote Control bridge mode");
                eprintln!("  Directory: {}", config.dir);
                eprintln!("  Branch: {}", config.branch);
                eprintln!("  Max sessions: {}", config.max_sessions);
                eprintln!("  Spawn mode: {:?}", config.spawn_mode);
                if let Some(sid) = session_id {
                    eprintln!("  Resuming session: {sid}");
                }

                eprintln!(
                    "\nBridge configured. Use `claude login` first, then connect via claude.ai/code."
                );
            }

            #[cfg(not(feature = "bridge"))]
            {
                let _ = (max_sessions, spawn_mode, session_id);
                eprintln!("Bridge mode is not compiled in. Rebuild with `--features bridge`.");
            }

            return Ok(());
        }
        Some(SubCommand::Config) => {
            let cwd = cli
                .working_dir
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            let root = claude_core::config::paths::detect_project_root(&cwd);
            println!("Project root: {}", root.display());
            println!(
                "Config dir: {}",
                claude_core::config::paths::claude_dir()?.display()
            );
            return Ok(());
        }
        None => {}
    }

    // ── Full bootstrap: settings, auth, MCP, skills, plugins, hooks ────

    // Initialize tracing before bootstrap (which logs)
    tracing_subscriber::fmt()
        .with_env_filter(if cli.verbose { "debug" } else { "error" })
        .init();

    let permission_mode = if cli.dangerously_skip_permissions {
        claude_core::permissions::types::PermissionMode::Bypass
    } else {
        claude_core::permissions::types::PermissionMode::Default
    };

    let bootstrap_config = claude_core::bootstrap::BootstrapConfig {
        working_dir: cli.working_dir.clone(),
        model: cli.model.clone(),
        verbose: cli.verbose,
        permission_mode: permission_mode.clone(),
        plan_mode: false,
        brief_mode: cli.brief,
        assistant_mode: cli.assistant,
        non_interactive: cli.prompt.is_some(),
        resume_session_id: cli.resume.clone(),
        custom_session_id: None,
        agent: None,
        append_system_prompt: cli.append_system_prompt.clone(),
        max_turns: cli.max_turns,
    };

    let bootstrap_result = claude_core::bootstrap::initialize(bootstrap_config).await?;
    let state_store = bootstrap_result.state;

    // Resolve auth from bootstrap result
    use claude_core::auth::resolve::AuthResolution;
    let auth_resolution = bootstrap_result.auth;

    let use_proxy = matches!(auth_resolution, AuthResolution::OAuthProxy);

    let auth = match auth_resolution {
        AuthResolution::ApiKey(auth) => auth,
        AuthResolution::OAuthToken(auth) => auth,
        AuthResolution::OAuthProxy => {
            claude_core::api::client::AuthMethod::ApiKey("proxy".into())
        }
        AuthResolution::None => {
            eprintln!();
            eprintln!("  \x1b[1mWelcome to Claude Code!\x1b[0m");
            eprintln!();
            if claude_core::api::claude_proxy::is_claude_available() {
                eprintln!("  Please run \x1b[1mclaude login\x1b[0m first, then try again.");
            } else {
                eprintln!("  To get started, either:");
                eprintln!("  1. Install Claude Code: \x1b[1mnpm install -g @anthropic-ai/claude-code\x1b[0m");
                eprintln!("     Then run: \x1b[1mclaude login\x1b[0m");
                eprintln!("  2. Or set: \x1b[1mexport ANTHROPIC_API_KEY=sk-ant-...\x1b[0m");
            }
            eprintln!();
            std::process::exit(1);
        }
    };

    // Read state for engine setup
    let project_root = state_store.read().project_root.clone();

    // Initialize session memory state (file setup already done by bootstrap)
    let session_memory_state = claude_core::services::SessionMemoryState::new();
    let _session_id = bootstrap_result.session_id.clone();

    // Initialize LSP manager (servers are lazily started when configs are provided)
    let lsp_manager = claude_core::services::LspManager::new();

    // Activate assistant mode if enabled (requires kairos feature)
    #[cfg(feature = "kairos")]
    let assistant_state = {
        let mut state = claude_core::assistant::AssistantState::default();
        if state_store.is_assistant_mode() {
            claude_core::assistant::activate_kairos(&mut state);
        }
        state
    };

    // Build tool registry
    let tools = claude_tools::build_default_registry();

    let model = state_store.active_model();

    tracing::info!(
        "claude-rs initialized: model={}, tools={}, project={}",
        model,
        tools.all().len(),
        project_root.display(),
    );

    // Build system prompt
    let tool_names: Vec<String> = tools.all().iter().map(|t| t.name().to_string()).collect();
    let memory_prompt = bootstrap_result.session_memory_content.as_deref();
    let system_prompt_values = claude_core::context::system_prompt::build_system_prompt_full(
        &project_root,
        &model,
        &tool_names,
        memory_prompt,
        None, // mcp_instructions
        None, // language_preference
    )
    .await?;

    // Append assistant-mode addendum if active (requires kairos feature)
    #[allow(unused_mut)]
    let mut system_prompt_values = system_prompt_values;
    #[cfg(feature = "kairos")]
    if assistant_state.is_active() {
        let addendum = claude_core::assistant::get_assistant_system_prompt_addendum();
        system_prompt_values.push(serde_json::json!({"type": "text", "text": addendum}));
    }

    // Convert Vec<Value> to Vec<ContentBlock> for the engine
    let system_prompt: Vec<claude_core::types::content::ContentBlock> = system_prompt_values
        .into_iter()
        .filter_map(|v| {
            v.get("text").and_then(|t| t.as_str()).map(|text| {
                claude_core::types::content::ContentBlock::Text {
                    text: text.to_string(),
                }
            })
        })
        .collect();

    // Create API client
    let model_display = model.clone();
    let api_config = claude_core::api::client::ApiConfig {
        model,
        task_budget: cli.task_budget,
        cache_ttl: if cli.cache_ttl > 0 {
            Some(cli.cache_ttl)
        } else {
            None
        },
        ..Default::default()
    };
    let api_client = claude_core::api::client::ApiClient::new(api_config, auth);

    // Create cancellation token
    let cancel = tokio_util::sync::CancellationToken::new();

    // Get tool definitions for the engine
    let tool_defs = tools.tool_definitions();

    // Create query engine
    let mut query_engine = claude_core::query::engine::QueryEngine::new(
        api_client,
        system_prompt,
        tool_defs,
        cancel.clone(),
    );

    // Wire AppState into the engine for live cost/usage/turn tracking
    query_engine.set_app_state(state_store.clone());

    if let Some(max) = cli.max_turns {
        query_engine.set_max_turns(max);
    }

    // Set query source: non-interactive (-p) vs interactive TUI
    if cli.prompt.is_some() {
        query_engine.set_query_source("cli_non_interactive".to_string());
    } else {
        query_engine.set_query_source("repl_main_thread".to_string());
    }

    // Handle non-interactive prompt mode
    if let Some(prompt) = cli.prompt {
        // If using OAuth proxy, delegate to real claude binary
        if use_proxy {
            let model_opt = Some(model_display.as_str());
            claude_core::api::claude_proxy::stream_via_claude(
                &prompt,
                model_opt,
                cancel.clone(),
                |text| print!("{}", text),
            )
            .await?;
            println!();
            return Ok(());
        }
        use claude_core::permissions::evaluator::evaluate_permission_sync;
        use claude_core::permissions::types::{PermissionBehavior, ToolPermissionContext};
        use claude_core::query::engine::TurnResult;
        use claude_core::types::events::StreamEvent;
        use claude_tools::ToolUseContext;
        use tokio::sync::mpsc;
        use std::sync::Arc;

        let cwd = state_store.read().cwd.clone();
        let perm_ctx = ToolPermissionContext {
            mode: permission_mode.clone(),
            ..Default::default()
        };

        // Wire streaming tool execution: create a ToolCallFn from the tool map
        {
            let tool_map: Arc<std::collections::HashMap<String, Arc<dyn claude_tools::ToolExecutor>>> =
                Arc::new(
                    tools
                        .all()
                        .into_iter()
                        .map(|t| (t.name().to_string(), t))
                        .collect(),
                );
            let cwd_clone = cwd.clone();
            let tool_call_fn: claude_core::query::tool_executor::ToolCallFn = Arc::new(
                move |name: String,
                      _id: String,
                      input: serde_json::Value,
                      cancel: tokio_util::sync::CancellationToken| {
                    let tool_map = Arc::clone(&tool_map);
                    let cwd = cwd_clone.clone();
                    tokio::spawn(async move {
                        let ctx = ToolUseContext::with_working_directory(cwd);
                        match tool_map.get(&name) {
                            Some(exec) => exec.call(&input, &ctx, cancel, None).await,
                            None => Ok(claude_core::types::events::ToolResultData {
                                data: serde_json::json!(format!("Unknown tool: {}", name)),
                                is_error: true,
                            }),
                        }
                    })
                },
            );
            query_engine.set_tool_call_fn(tool_call_fn);
        }

        query_engine.add_user_message(&prompt);

        // Run the agentic loop: prompt → run_turn → ToolUse* → Done
        loop {
            let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(128);

            // Spawn a task to print streamed text to stdout
            let print_handle = tokio::spawn(async move {
                while let Some(ev) = stream_rx.recv().await {
                    match ev {
                        StreamEvent::TextDelta { text } => {
                            print!("{}", text);
                        }
                        StreamEvent::Done { .. } => {
                            println!();
                        }
                        _ => {}
                    }
                }
            });

            let result = query_engine.run_turn(&stream_tx).await?;
            drop(stream_tx);
            let _ = print_handle.await;

            match result {
                TurnResult::Done(_) => {
                    let token_count = claude_core::services::rough_token_count_estimation(
                        &serde_json::to_string(query_engine.messages()).unwrap_or_default(),
                        4,
                    ) as u64;
                    if session_memory_state.has_met_init_threshold(token_count) {
                        if !session_memory_state.is_initialized() {
                            session_memory_state.mark_initialized();
                        }
                        if session_memory_state.has_met_update_threshold(token_count) {
                            session_memory_state.record_extraction_token_count(token_count);
                            tracing::debug!(token_count, "session memory extraction threshold met");
                        }
                    }
                    break;
                }
                TurnResult::ContinueRecovery
                | TurnResult::StopHookBlocking
                | TurnResult::TokenBudgetContinuation => {
                    // Recovery/continuation — run again immediately
                    continue;
                }
                TurnResult::ToolUse(tool_uses) => {
                    // Execute each tool, check permissions, feed results back
                    for tool_info in &tool_uses {
                        let is_read_only = tools
                            .get(&tool_info.name)
                            .map(|t| t.is_read_only(&tool_info.input))
                            .unwrap_or(false);

                        let decision = evaluate_permission_sync(
                            &tool_info.name,
                            &tool_info.input,
                            &perm_ctx,
                            is_read_only,
                        );

                        let (result_text, is_error) = match decision.behavior {
                            PermissionBehavior::Allow | PermissionBehavior::Ask => {
                                // In non-interactive mode, auto-allow (user passed a prompt)
                                let executor = tools.get(&tool_info.name);
                                match executor {
                                    Some(exec) => {
                                        let ctx =
                                            ToolUseContext::with_working_directory(cwd.clone());
                                        match exec
                                            .call(&tool_info.input, &ctx, cancel.clone(), None)
                                            .await
                                        {
                                            Ok(data) => {
                                                let text = data
                                                    .data
                                                    .as_str()
                                                    .unwrap_or(&data.data.to_string())
                                                    .to_string();
                                                (text, data.is_error)
                                            }
                                            Err(e) => (format!("Error: {}", e), true),
                                        }
                                    }
                                    None => (format!("Unknown tool: {}", tool_info.name), true),
                                }
                            }
                            PermissionBehavior::Deny => {
                                let message = decision.message.unwrap_or_else(|| "Denied".to_string());
                                (format!("Permission denied: {}", message), true)
                            }
                        };

                        query_engine.add_tool_result(&tool_info.id, &result_text, is_error);

                        // Notify LSP of file saves after Edit/Write tools
                        if matches!(tool_info.name.as_str(), "Edit" | "Write") {
                            if let Some(path) = tool_info.input.get("file_path").and_then(|v| v.as_str()) {
                                let _ = lsp_manager.save_file(path);
                            }
                        }
                    }
                    // Continue the loop to call run_turn again with the tool results
                }
            }
        }

        return Ok(());
    }

    // Interactive TUI mode
    if use_proxy {
        // For OAuth users, launch the real claude binary in interactive mode
        // since we can't call the API directly
        let status = std::process::Command::new("claude")
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to launch claude: {}", e))?;
        std::process::exit(status.code().unwrap_or(1));
    }
    let mut app = claude_tui::app::App::new()?;
    app.set_model_name(&model_display);
    app.set_app_state(state_store.clone());
    let perm_mode = state_store.read().permission_mode.clone();
    app.run_with_engine(query_engine, tools, cancel, perm_mode).await?;

    Ok(())
}
