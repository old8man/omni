# Omni

A high-performance, fully-unlocked reimplementation of Claude Code in Rust.

All features enabled by default. No feature gates. No entitlement checks. No rate-limit tiers. Full access to every capability — including internal and experimental features that are restricted in the original.

## What's different from Claude Code

| | Claude Code (original) | Omni |
|---|---|---|
| Language | TypeScript/Node.js | Rust |
| Startup time | ~2s | ~50ms |
| Memory usage | ~200MB | ~15MB |
| Feature gates | GrowthBook, entitlements, compile-time flags | None — everything enabled |
| KAIROS (always-on assistant) | Anthropic-only | Available to everyone |
| Coordinator mode | Anthropic-only | Available to everyone |
| Dream mode | Anthropic-only | Available to everyone |
| Brief mode | Gated | Always available |
| Voice input | Gated | Always available |
| Reactive compaction | Gated | Always available |
| Auto-mode classifier | Gated | Always available |
| Remote triggers / cron | Gated | Always available |
| All internal commands | Hidden | Fully accessible |

## Architecture

Four crates, zero bloat:

| Crate | Lines | Purpose |
|-------|-------|---------|
| `omni-core` | ~22k | API client, query engine, auth, MCP, bridge, remote, IDE, voice, KAIROS, compaction, sessions, hooks, skills, plugins, permissions, cost tracking |
| `omni-tools` | ~8k | 34+ tool implementations — Bash, Read, Write, Edit, Grep, Glob, WebSearch, WebFetch, Agent, Team, Tasks, MCP, REPL, LSP, and more |
| `omni-tui` | ~5k | Interactive terminal UI — ratatui, vim mode, syntax highlighting, mouse support, search, keybindings |
| `omni-cli` | ~1k | Binary entry point, CLI parsing, orchestration |

**~36k lines of Rust** replacing ~500k lines of TypeScript.

## Features

### Core Engine
- Full agentic loop with multi-turn tool use and streaming
- Real-time SSE streaming from the Anthropic Messages API
- Automatic retry with exponential backoff (429, 500, 502, 503, 504, 529)
- Message normalization before every API call
- Max-tokens recovery (escalation 8k → 64k, then retry loop)
- Synthetic tool_results on cancellation (prevents conversation corruption)
- Token budget tracking across turns

### Compaction
- **Auto-compact**: API-based summarization with 9-section structured prompt
- **Micro-compact**: truncate large tool results inline (no API call)
- **Snip-compact**: drop oldest messages, keep recent
- **Reactive-compact**: two-phase recovery on prompt-too-long errors
- Compact boundary markers, post-compact file restoration
- Per-turn microcompact as preprocessing pass

### Context
- CLAUDE.md auto-discovery (project, user, parent directories, rules/)
- Memory system (~/.claude/projects/{project}/memory/)
- 685-line system prompt matching production Claude Code exactly
- Model-specific knowledge cutoffs and marketing names
- Git context injection (branch, commits, status)

### Tools (34+)
Bash, Read, Write, Edit, Grep, Glob, WebSearch, WebFetch, Sleep, AskUser, ToolSearch, NotebookEdit, LSP, Agent, TeamCreate, TeamDelete, SendMessage, TaskCreate/List/Get/Update/Stop/Output, Skill, MCP, EnterPlanMode, ExitPlanMode, EnterWorktree, ExitWorktree, Brief (SendUserMessage), Config, REPL, PowerShell, SyntheticOutput, TodoWrite, RemoteTrigger, ScheduleCron

### Commands (47+)
help, clear, compact, config, status, model, resume, version, usage, quit, doctor, diff, memory, plan, vim, context, commit, commit-push-pr, review, security-review, login, logout, effort, hooks, permissions, theme, session, branch, init, fast, export, feedback, tasks, voice, skills, keybindings, brief, advisor, copy, rename, rewind, stats, sandbox, upgrade, tag, stickers, thinkback

### MCP (Model Context Protocol)
- Full JSON-RPC 2.0 client over stdio
- Multi-server manager with aggregated tool discovery
- Config loading from ~/.claude/mcp.json + .mcp.json
- Env var expansion with ${VAR:-default} syntax
- Ping/pong keepalive, tools/list_changed notifications
- Tool annotations (readOnly, destructive, openWorld)

### KAIROS (Always-On Assistant)
- `--assistant` mode: proactive, autonomous operation
- Tick-based scheduling with configurable intervals
- Brief mode: ultra-concise output via SendUserMessage tool
- Daily logging with categorized entries (Observation/Decision/Action)
- Dream mode: memory consolidation during idle periods
- Push notifications and PR subscription tracking

### TUI
- Ratatui-based with 60fps rendering
- Full vim mode (motions, operators, text objects, dot-repeat, visual mode)
- Syntax highlighting for 15+ languages (via syntect)
- GFM markdown rendering (tables, code blocks, links, blockquotes, nested lists)
- OSC 8 clickable hyperlinks
- Mouse support (scroll, click-to-focus, drag selection)
- Clipboard integration (OSC 52 + native fallback)
- Ctrl+F search with match navigation
- Keybinding system with chord support and user overrides
- Permission dialogs, spinner, status bar, notifications
- Agent status panel, context visualization, diff viewer
- Light/dark theme auto-detection

### Auth
- API key via ANTHROPIC_API_KEY
- OAuth PKCE flow with browser redirect
- macOS Keychain integration
- Token refresh with expiry detection
- API key verification

### Bridge / Remote / IDE
- Bridge mode for claude.ai integration (HTTP polling + heartbeat)
- Remote session management via WebSocket
- IDE integration (VS Code, JetBrains) via Unix socket / JSON-RPC
- JWT parsing and validation

### Voice
- Streaming speech-to-text with provider abstraction
- Configurable audio parameters (16kHz, mono)
- Silence detection and wake-word support

### Sessions & History
- Auto-save after every turn
- Session resume via --resume
- Append-only history at ~/.claude/history.jsonl
- Session listing with summaries

### Skills & Plugins
- 13 bundled skills (simplify, commit, review, api, loop, schedule, verify, debug, remember, stuck, batch, skillify, loremIpsum)
- Markdown-with-YAML-frontmatter skill format
- Plugin discovery from ~/.claude/plugins/ and .claude/plugins/
- Plugin manifest parsing (package.json / plugin.json)

### Additional
- Cost tracking with per-model pricing for all Claude models
- Hook system (27 event types, shell + HTTP execution)
- Git worktree management (create, enter, exit, list)
- Settings from multiple sources (user + project + local) with merge
- Migration system for config upgrades
- Auto-updater, release notes, diagnostics
- Prevent-sleep (caffeinate on macOS)

## Building

```bash
cargo build --release
```

## Usage

```bash
# Interactive TUI
omni

# Single prompt
omni "explain this codebase"

# With model selection
omni -m claude-sonnet-4-6 "fix the bug"

# Always-on assistant mode (KAIROS)
omni --assistant

# Brief mode
omni --brief

# Bridge mode (for claude.ai integration)
omni remote-control

# Resume previous session
omni --resume <session-id>

# Non-interactive with max turns
omni --max-turns 10 "refactor the auth module"
```

## Configuration

```
~/.claude/settings.json     — user settings
.claude/settings.json       — project settings
~/.claude/mcp.json          — MCP server config
.mcp.json                   — project MCP config
~/.claude/keybindings.json  — custom keybindings
CLAUDE.md                   — project context
.claude/CLAUDE.md           — project context (alt)
~/.claude/CLAUDE.md         — user context
```

## License

MIT
