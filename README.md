# 🦀 Agent CLI MCP Server (Rust)

A high-performance, security-first **Model Context Protocol (MCP)** server written in Rust that orchestrates multiple AI coding agents through a unified interface.

> **One server. Six executors. Full bidirectional control.**

## Supported Executors

| Executor | Binary | Status |
|---|---|---|
| **GitHub Copilot CLI** | `copilot` | Full support — prompt, autopilot, fleet, delegate, review, keep-alive |
| **Google Jules** | `jules` | Full support — remote sessions, plan approval, API, verification |
| **Gemini CLI** | `gemini` | Generic executor — prompt, interactive, run |
| **OpenAI Codex CLI** | `codex` | Generic executor — prompt, interactive, run |
| **OpenCode CLI** | `opencode` | Generic executor — prompt, interactive, run |
| **Anthropic Claude CLI** | `claude` | Generic executor — prompt, interactive, run |

---

## Architecture

```
┌────────────────────────────────────────────────┐
│              MCP Client (Orchestrator)          │
│   Claude Code · Gemini · Codex · Cursor · etc  │
└───────────────────┬────────────────────────────┘
                    │ JSON-RPC over stdio
                    ▼
┌────────────────────────────────────────────────┐
│         agent-cli-mcp-rust (this server)       │
│                                                │
│  ┌──────────┐ ┌──────────┐ ┌──────────────┐   │
│  │ Policy   │ │ Redaction│ │ Session Mgr  │   │
│  │ Engine   │ │ Engine   │ │ (async I/O)  │   │
│  └──────────┘ └──────────┘ └──────────────┘   │
│                                                │
│  ┌──────────────────────────────────────────┐  │
│  │           Capability Discovery           │  │
│  │  Copilot · Jules · Gemini · Codex ·      │  │
│  │  OpenCode · Claude                       │  │
│  └──────────────────────────────────────────┘  │
└──────┬──────┬──────┬──────┬──────┬──────┬──────┘
       │      │      │      │      │      │
       ▼      ▼      ▼      ▼      ▼      ▼
   Copilot  Jules  Gemini  Codex  Open   Claude
    CLI     CLI    CLI     CLI    Code    CLI
```

**Bidirectional communication**: The server maintains persistent sessions with stdin/stdout pipes, enabling real-time interactive exchanges between the orchestrator and any executor.

---

## Installation

### Prerequisites

- **Rust 1.75+** (install via [rustup](https://rustup.rs))
- At least one supported CLI installed and on your `$PATH`

### Build from Source

```bash
git clone https://github.com/your-org/agent-cli-mcp-rust.git
cd agent-cli-mcp-rust
cargo build --release
```

The binary will be at `target/release/agent-cli-mcp-rust`.

### Quick Verify

```bash
# Check it starts and responds to MCP initialization
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}' \
  | ./target/release/agent-cli-mcp-rust 2>/dev/null | head -1
```

---

## Configuration

Configuration is loaded with the following precedence (highest wins):

1. **Environment variables** (e.g. `COPILOT_BIN`, `GEMINI_BIN`)
2. **Config file** (pointed to by `AGENT_CLI_CONFIG`)
3. **Built-in defaults**

### Config File

Copy the included [`config.json`](config.json) template:

```bash
cp config.json ~/.agent-cli-mcp/config.json
export AGENT_CLI_CONFIG=~/.agent-cli-mcp/config.json
```

```jsonc
{
  // Directories the server is allowed to operate in
  "allowedRoots": ["~/Dev", "~/projects"],

  // State directory for run logs and session tracking
  "stateDir": "~/.agent-cli-mcp",

  // Default timeout for executor processes (30 minutes)
  "defaultTimeoutMs": 1800000,

  // Maximum characters returned in stdout/stderr output
  "maxOutputChars": 50000,

  // CLI binary paths (override if not on $PATH)
  "copilotBin": "copilot",
  "julesBin": "jules",
  "ghBin": "gh",
  "geminiBin": "gemini",
  "codexBin": "codex",
  "opencodeBin": "opencode",
  "claudeBin": "claude"
}
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `AGENT_CLI_CONFIG` | — | Path to JSON config file |
| `AGENT_CLI_ALLOWED_ROOTS` | `~/Dev,~/dev,~/projects` | Comma-separated allowed directories |
| `AGENT_CLI_STATE_DIR` | `~/.agent-cli-mcp` | State/log storage directory |
| `AGENT_CLI_DEFAULT_TIMEOUT_MS` | `1800000` | Default process timeout |
| `AGENT_CLI_MAX_OUTPUT_CHARS` | `50000` | Max output chars returned |
| `COPILOT_BIN` | `copilot` | Path to Copilot CLI |
| `JULES_BIN` | `jules` | Path to Jules CLI |
| `GH_BIN` | `gh` | Path to GitHub CLI |
| `GEMINI_BIN` | `gemini` | Path to Gemini CLI |
| `CODEX_BIN` | `codex` | Path to Codex CLI |
| `OPENCODE_BIN` | `opencode` | Path to OpenCode CLI |
| `CLAUDE_BIN` | `claude` | Path to Claude CLI |
| `JULES_API_KEY` | — | Jules API key for API mode |
| `JULES_API_KEY_CMD` | — | Command to retrieve Jules API key |
| `GITHUB_TOKEN_CMD` | — | Command to retrieve GitHub token |
| `AGENT_CLI_WORKTREE_ROOT` | — | Custom worktree storage root |

---

## Registering with MCP Clients

### Claude Code / Claude Desktop

Add to your MCP settings (e.g. `~/.claude/settings.json` or `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "agent-cli": {
      "command": "/path/to/agent-cli-mcp-rust",
      "env": {
        "AGENT_CLI_ALLOWED_ROOTS": "~/Dev",
        "AGENT_CLI_CONFIG": "~/.agent-cli-mcp/config.json"
      }
    }
  }
}
```

### Gemini CLI / Antigravity

Add to your Gemini MCP configuration:

```json
{
  "mcpServers": {
    "agent-cli": {
      "command": "/path/to/agent-cli-mcp-rust",
      "env": {
        "AGENT_CLI_ALLOWED_ROOTS": "~/Dev"
      }
    }
  }
}
```

### Cursor / VS Code

Configure in your editor's MCP server settings following the same pattern.

---

## Security Model

### Directory Isolation

All commands are restricted to directories under `allowedRoots`. The server:
- Resolves symlinks and canonicalizes paths before comparison
- Validates `cwd` against allowed roots on every tool call
- Supports quarantine markers (`.agent-cli-quarantine`) to freeze directories

### Secret Redaction

All output passes through a strict redaction engine that scrubs:
- API keys and tokens (Bearer, sk-, ghp_, ghu_, etc.)
- AWS credentials, Azure keys, GCP service accounts
- Database connection strings with credentials
- JWT tokens, private keys, and passphrases

### Tool Permission Profiles

Built-in presets enforce least-privilege access:

| Profile | Use Case |
|---|---|
| `copilot-file-edit` | Scoped file edits only |
| `copilot-safe-dev` | Dev commands + file edits |
| `copilot-expanded-worktree` | Full worktree access |
| `copilot-target-repo` | Full repo with build tools |

### Mandatory Deny Overlay

These operations are **always blocked**, regardless of profile:
- `memory` — persistent memory writes
- `vercel deploy --prod` — production deployments
- `supabase db reset` — destructive DB operations
- `git push --force` — force pushes
- `security find-generic-password` — macOS Keychain access

---

## Tool Reference

### Discovery & Health

| Tool | Description |
|---|---|
| `agent_cli.overview` | Compact dashboard of all agent activity |
| `agent_cli.capabilities` | Probe CLIs for supported flags and versions |
| `agent_cli.executor_health` | Health check across all executors |
| `agent_cli.test_executor_profile` | Smoke-test a tool permission profile |

### Execution

| Tool | Description |
|---|---|
| `agent_cli.run` | One-shot executor dispatch (waits for completion) |
| `agent_cli.run_quick` | Short-lived run with immediate output |
| `agent_cli.start_run` | Background run (returns immediately) |
| `agent_cli.start_session` | Long-running interactive session |
| `agent_cli.send_input` | Write to a session's stdin |
| `agent_cli.read_output` | Read captured stdout/stderr |
| `agent_cli.list_sessions` | List runs and active sessions |
| `agent_cli.kill_session` | Terminate a running process |
| `agent_cli.run_binary_scoped` | L3 escape hatch with sandbox scoping |

### Copilot-Specific

| Tool | Description |
|---|---|
| `copilot.run` | Convenience wrapper for Copilot runs |
| `copilot.fleet` | Run Copilot `/fleet` mode |
| `copilot.delegate` | Copilot `/delegate` for autonomous work |
| `copilot.autopilot` | Copilot autopilot mode |
| `copilot.keep_alive` | Long-running `/keep-alive` session |
| `copilot.review` | Copilot `/review` for change review |

### Jules-Specific

| Tool | Description |
|---|---|
| `jules.create_session` | Create a Jules API session |
| `jules.list_sessions` | List Jules sessions |
| `jules.get_session` | Fetch a single session |
| `jules.get_status` | Normalized status with activities/plan/outputs |
| `jules.approve_plan` | Approve the latest plan |
| `jules.send_message` | Send feedback to a session |
| `jules.request_verification` | Ask Jules to verify results |
| `jules.pending_actions` | Sessions needing attention |
| `jules.watch` | Poll session state changes |
| `jules.collect_outputs` | Collect session outputs |

### Infrastructure

| Tool | Description |
|---|---|
| `agent_cli.create_worktree` | Create isolated git worktrees |
| `agent_cli.resolve_repo_context` | Resolve repo/worktree context |
| `agent_cli.sanity_check` | Scan for mutations or leaks |
| `agent_cli.quarantine` | Freeze a directory |
| `agent_cli.collect_artifacts` | Collect executor outputs |
| `gh.pr_list` | List open pull requests |

### Resources

| URI | Description |
|---|---|
| `agent://overview` | Dashboard as markdown |
| `agent://running` | Currently running sessions |
| `agent://pending-input` | Sessions awaiting input |
| `agent://done` | Completed sessions |
| `agent://stale` | Failed/stale sessions |
| `jules://sessions` | All Jules sessions |
| `jules://session/{id}` | Single Jules session |
| `jules://pending-actions` | Jules sessions needing action |

---

## Development

```bash
# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run

# Format code
cargo fmt

# Lint
cargo clippy
```

---

## License

MIT
