use serde_json::json;
use std::sync::Arc;

use crate::config::Config;
use crate::errors::{AgentCliError, ErrorCode};
use crate::jules_client;
use crate::mcp::McpServer;
use crate::policy;
use crate::runner::SessionManager;
use crate::sanity;
use crate::store::Store;
use crate::tools;

pub fn register_all(
    server: &McpServer,
    config: Arc<Config>,
    store: Arc<Store>,
    session_manager: Arc<SessionManager>,
) {
    // ── Resources ────────────────────────────────────────────────────────────

    // jules://sessions
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "jules-sessions",
        "jules://sessions",
        "Jules Sessions",
        "Current Jules sessions and their normalized status",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let args = tools::ListSessionsArgs {
                    include_activities: Some(false),
                    page_size: Some(50),
                };
                let res = tools::list_jules_sessions(args, &config, &store).await?;
                Ok(res)
            }
        },
    );

    // jules://session/{sessionId}
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "jules-session",
        "jules://session/{sessionId}",
        "Jules Session",
        "Status snapshot for a single Jules session",
        "application/json",
        move |_, params| {
            let config = c.clone();
            let store = s.clone();
            let session_id = params.get("sessionId").cloned().unwrap_or_default();
            async move {
                let args = tools::GetJulesStatusArgs {
                    session_id,
                    include_activities: Some(true),
                    include_plan: Some(true),
                    include_outputs: Some(true),
                };
                let res = tools::get_jules_status(args, &config, &store).await?;
                Ok(res)
            }
        },
    );

    // jules://session/{sessionId}/activities
    let c = config.clone();
    server.register_resource(
        "jules-session-activities",
        "jules://session/{sessionId}/activities",
        "Jules Session Activities",
        "Activity feed for a single Jules session",
        "application/json",
        move |_, params| {
            let config = c.clone();
            let session_id = params.get("sessionId").cloned().unwrap_or_default();
            async move {
                let client = jules_client::JulesClient::new(&config)?;
                let activities = client.list_activities(&session_id, 100, None).await?;
                Ok(json!({ "activities": activities }))
            }
        },
    );

    // jules://pending-actions
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "jules-pending-actions",
        "jules://pending-actions",
        "Jules Pending Actions",
        "Jules sessions that need approval, feedback, verification, or failure handling",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = tools::list_jules_pending_actions(&config, &store).await?;
                Ok(res)
            }
        },
    );

    // jules://stale-runs
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "jules-stale-runs",
        "jules://stale-runs",
        "Jules Stale Runs",
        "Local Jules bookkeeping entries that do not correspond to a live remote session",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = tools::reconcile_local_jules_runs(&config, &store).await?;
                Ok(res)
            }
        },
    );

    // agent://overview
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "agent-overview",
        "agent://overview",
        "Agent CLI Overview",
        "Compact dashboard: running, awaiting-input, done, failed/stale sessions and executor health",
        "text/markdown",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = crate::tools_overview::get_overview(None, false, false, &config, &store).await?;
                Ok(json!({ "contents": [{ "uri": "agent://overview", "text": res.markdown, "mimeType": "text/markdown" }] }))
            }
        },
    );

    // agent://running
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "agent-running",
        "agent://running",
        "Running Agents",
        "Sessions and runs currently in progress",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = crate::tools_overview::get_overview(None, false, false, &config, &store).await?;
                let text = serde_json::to_string_pretty(&res.running).unwrap();
                Ok(json!({ "contents": [{ "uri": "agent://running", "text": text, "mimeType": "application/json" }] }))
            }
        },
    );

    // agent://pending-input
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "agent-pending-input",
        "agent://pending-input",
        "Agents Awaiting Input",
        "Sessions needing plan approval, feedback, or verification",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = crate::tools_overview::get_overview(None, false, false, &config, &store).await?;
                let text = serde_json::to_string_pretty(&res.awaiting_input).unwrap();
                Ok(json!({ "contents": [{ "uri": "agent://pending-input", "text": text, "mimeType": "application/json" }] }))
            }
        },
    );

    // agent://done
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "agent-done",
        "agent://done",
        "Completed Agents",
        "Recently completed sessions",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = crate::tools_overview::get_overview(None, true, false, &config, &store).await?;
                let text = serde_json::to_string_pretty(&res.done).unwrap();
                Ok(json!({ "contents": [{ "uri": "agent://done", "text": text, "mimeType": "application/json" }] }))
            }
        },
    );

    // agent://stale
    let c = config.clone();
    let s = store.clone();
    server.register_resource(
        "agent-stale",
        "agent://stale",
        "Stale / Failed Agents",
        "Sessions that failed or have no corresponding remote state",
        "application/json",
        move |_, _| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = crate::tools_overview::get_overview(None, false, false, &config, &store).await?;
                let text = serde_json::to_string_pretty(&res.failed_or_stale).unwrap();
                Ok(json!({ "contents": [{ "uri": "agent://stale", "text": text, "mimeType": "application/json" }] }))
            }
        },
    );

    // ── Tools ────────────────────────────────────────────────────────────────

    // agent_cli.overview
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.overview",
        "Compact dashboard showing all agent activity: running sessions, sessions awaiting input...",
        json!({
            "type": "object",
            "properties": {
                "repo": { "type": "string", "description": "Filter by repository path" },
                "includeDone": { "type": "boolean", "description": "Include completed runs" },
                "includeLogs": { "type": "boolean", "description": "Include log summaries" }
            }
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let repo = args.get("repo").and_then(|v| v.as_str()).map(|s| s.to_string());
                let include_done = args.get("includeDone").and_then(|v| v.as_bool()).unwrap_or(false);
                let include_logs = args.get("includeLogs").and_then(|v| v.as_bool()).unwrap_or(false);
                let res = crate::tools_overview::get_overview(repo, include_done, include_logs, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": res.markdown }] }))
            }
        },
    );

    // agent_cli.capabilities
    let c = config.clone();
    server.register_tool(
        "agent_cli.capabilities",
        "Probe installed CLIs to discover supported flags and versions.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude", "all"], "default": "all" },
                "refresh": { "type": "boolean", "default": false }
            }
        }),
        move |args| {
            let config = c.clone();
            async move {
                let service = args.get("service").and_then(|v| v.as_str()).unwrap_or("all");
                let refresh = args.get("refresh").and_then(|v| v.as_bool()).unwrap_or(false);
                let res = tools::discover_capabilities(service, &config, refresh).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.executor_health
    let c = config.clone();
    server.register_tool(
        "agent_cli.executor_health",
        "Check whether executor CLIs are installed and healthy.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude", "all"], "default": "all" }
            }
        }),
        move |args| {
            let config = c.clone();
            async move {
                let service = args.get("service").and_then(|v| v.as_str()).unwrap_or("all");
                let res = tools::executor_health(service, &config).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.test_executor_profile
    let c = config.clone();
    server.register_tool(
        "agent_cli.test_executor_profile",
        "Smoke-test a named tool-permission profile from TOOL_PRESETS.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude", "all"], "default": "all" },
                "profile": { "type": "string" }
            },
            "required": ["profile"]
        }),
        move |args| {
            let config = c.clone();
            async move {
                let service = args.get("service").and_then(|v| v.as_str()).unwrap_or("all");
                let profile = args.get("profile").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'profile'")
                })?;
                let res = tools::test_executor_profile(service, profile, &config).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.run
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.run",
        "Dispatch a one-shot executor run via any supported CLI.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude"] },
                "mode": { "type": "string" },
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" },
                "dryRun": { "type": "boolean" }
            },
            "required": ["service", "mode", "cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::RunInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::execute_run(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.run_quick
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.run_quick",
        "Dispatch a short-lived executor run and wait for completion.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude"] },
                "mode": { "type": "string" },
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" },
                "dryRun": { "type": "boolean" }
            },
            "required": ["service", "mode", "cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::RunInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::execute_run(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.start_run
    let c = config.clone();
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "agent_cli.start_run",
        "Start a background executor run and return immediately.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude"] },
                "mode": { "type": "string" },
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" },
                "dryRun": { "type": "boolean" }
            },
            "required": ["service", "mode", "cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::RunInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::start_run(input, &config, &store, &sm).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.start_session
    let c = config.clone();
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "agent_cli.start_session",
        "Start a long-running interactive executor process.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude"] },
                "cwd": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["service", "cwd", "argv"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::StartSessionInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::start_session(input, &config, &store, &sm).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.send_input
    let sm = session_manager.clone();
    server.register_tool(
        "agent_cli.send_input",
        "Write text to a running session's stdin.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "input": { "type": "string" }
            },
            "required": ["sessionId", "input"]
        }),
        move |args| {
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::SendInputArgs>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::send_input(input, &sm).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.read_output
    let c = config.clone();
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "agent_cli.read_output",
        "Read captured stdout/stderr from a run or session.",
        json!({
            "type": "object",
            "properties": {
                "runId": { "type": "string" },
                "sessionId": { "type": "string" },
                "stream": { "type": "string", "enum": ["stdout", "stderr", "both"] },
                "mode": { "type": "string", "enum": ["tail", "full"] },
                "maxChars": { "type": "integer" }
            }
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::ReadOutputArgs>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::read_output(input, &config, &store, &sm)?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.list_sessions
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "agent_cli.list_sessions",
        "List recent runs and active sessions.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gemini", "codex", "opencode", "claude", "all"] },
                "status": { "type": "string", "enum": ["running", "complete", "failed", "killed", "all"] },
                "limit": { "type": "integer" }
            }
        }),
        move |args| {
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::ListSessionsInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::list_sessions(input, &store, &sm)?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.kill_session
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "agent_cli.kill_session",
        "Kill a running executor process.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "reason": { "type": "string" }
            },
            "required": ["sessionId"]
        }),
        move |args| {
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::KillSessionArgs>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::kill_session(input, &store, &sm).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.create_worktree
    let c = config.clone();
    server.register_tool(
        "agent_cli.create_worktree",
        "Create an isolated git worktree for a task.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "branch": { "type": "string" },
                "base": { "type": "string" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            async move {
                let input = serde_json::from_value::<tools::CreateWorktreeInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::create_worktree(input, &config).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.resolve_repo_context
    let _c = config.clone();
    server.register_tool(
        "agent_cli.resolve_repo_context",
        "Resolve current repo/worktree context.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            async move {
                let cwd = args.get("cwd").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'cwd'")
                })?;
                let res = policy::detect_git_context(cwd).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::BackendFailed, "Failed to detect git context")
                })?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.sanity_check
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.sanity_check",
        "Scan working directory or logs for mutations or leaks.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "runId": { "type": "string" },
                "sessionId": { "type": "string" },
                "checkDiff": { "type": "boolean" },
                "checkLogs": { "type": "boolean" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let cwd = args.get("cwd").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'cwd'")
                })?;
                let run_id = args.get("runId").and_then(|v| v.as_str()).map(|s| s.to_string());
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).map(|s| s.to_string());
                let check_run_id = run_id.or(session_id);
                let check_diff = args.get("checkDiff").and_then(|v| v.as_bool()).unwrap_or(true);
                let check_logs = args.get("checkLogs").and_then(|v| v.as_bool()).unwrap_or(true);

                let res = sanity::run_sanity_check(cwd, check_run_id.as_deref(), Some(check_diff), Some(check_logs), &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.quarantine
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.quarantine",
        "Freeze a directory from further executor dispatch.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "reason": { "type": "string" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::QuarantineInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::quarantine_directory(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.collect_artifacts
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.collect_artifacts",
        "Collect executor outputs from a working directory.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "runId": { "type": "string" },
                "sessionId": { "type": "string" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::CollectArtifactsInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::collect_artifacts(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // copilot.run
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "copilot.run",
        "Convenience wrapper for copilot runs.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "mode": { "type": "string" },
                "prompt": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } },
                "allowTools": { "type": "array", "items": { "type": "string" } },
                "denyTools": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" },
                "dryRun": { "type": "boolean" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::CopilotRunInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::copilot_run(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // copilot.fleet
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "copilot.fleet",
        "Run a Copilot /fleet prompt.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "allowTools": { "type": "array", "items": { "type": "string" } },
                "denyTools": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::CopilotFleetInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::copilot_fleet(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // copilot.delegate
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "copilot.delegate",
        "Run Copilot /delegate for autonomous delegation.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "allowTools": { "type": "array", "items": { "type": "string" } },
                "denyTools": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::CopilotDelegateInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::copilot_delegate(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // copilot.autopilot
    let c = config.clone();
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "copilot.autopilot",
        "Run Copilot in autopilot mode.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "allowTools": { "type": "array", "items": { "type": "string" } },
                "denyTools": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" },
                "background": { "type": "boolean" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::CopilotAutopilotInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::copilot_autopilot(input, &config, &store, &sm).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // copilot.keep_alive
    let c = config.clone();
    let s = store.clone();
    let sm = session_manager.clone();
    server.register_tool(
        "copilot.keep_alive",
        "Start a long-running Copilot /keep-alive session.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "allowTools": { "type": "array", "items": { "type": "string" } },
                "denyTools": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            let sm = sm.clone();
            async move {
                let input = serde_json::from_value::<tools::CopilotKeepAliveInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::copilot_keep_alive(input, &config, &store, &sm).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // copilot.review
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "copilot.review",
        "Run Copilot /review to review changes.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "allowTools": { "type": "array", "items": { "type": "string" } },
                "denyTools": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" },
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::CopilotReviewInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::copilot_review(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // agent_cli.run_binary_scoped
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "agent_cli.run_binary_scoped",
        "Level-3 escape hatch scoped runner.",
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "enum": ["copilot", "jules", "gh", "gemini", "codex", "opencode", "claude"] },
                "cwd": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } },
                "sandboxLevel": { "type": "string", "enum": ["target-repo", "isolated-worktree", "read-only"] },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["service", "cwd", "argv", "sandboxLevel"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::RunBinaryScopedInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::run_binary_scoped(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.list_sources
    let c = config.clone();
    server.register_tool(
        "jules.list_sources",
        "List Jules API sources.",
        json!({
            "type": "object"
        }),
        move |_| {
            let config = c.clone();
            async move {
                let res = tools::list_jules_sources(&config).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.create_session
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.create_session",
        "Create a Jules API session.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "source": { "type": "string" },
                "startingBranch": { "type": "string" },
                "title": { "type": "string" },
                "requirePlanApproval": { "type": "boolean" },
                "automationMode": { "type": "string" },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::JulesCreateSessionInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::create_jules_session(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.list_sessions
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.list_sessions",
        "List Jules sessions.",
        json!({
            "type": "object",
            "properties": {
                "includeActivities": { "type": "boolean" },
                "pageSize": { "type": "integer" }
            }
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::ListSessionsArgs>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::list_jules_sessions(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.get_session
    let c = config.clone();
    server.register_tool(
        "jules.get_session",
        "Fetch a single Jules session.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" }
            },
            "required": ["sessionId"]
        }),
        move |args| {
            let config = c.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let client = jules_client::JulesClient::new(&config)?;
                let res = client.get_session(session_id).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.list_activities
    let c = config.clone();
    server.register_tool(
        "jules.list_activities",
        "List activities for a Jules session.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "pageSize": { "type": "integer" }
            },
            "required": ["sessionId"]
        }),
        move |args| {
            let config = c.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let page_size = args.get("pageSize").and_then(|v| v.as_u64()).unwrap_or(50) as i32;
                let client = jules_client::JulesClient::new(&config)?;
                let res = client.list_activities(session_id, page_size, None).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.get_status
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.get_status",
        "Get normalized Jules status.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "includeActivities": { "type": "boolean" },
                "includePlan": { "type": "boolean" },
                "includeOutputs": { "type": "boolean" }
            },
            "required": ["sessionId"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::GetJulesStatusArgs>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::get_jules_status(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.approve_plan
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.approve_plan",
        "Approve the latest Jules plan.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" }
            },
            "required": ["sessionId"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let res = tools::approve_jules_plan(session_id, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.send_message
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.send_message",
        "Send feedback to an active Jules session.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "prompt": { "type": "string" }
            },
            "required": ["sessionId", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let prompt = args.get("prompt").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'prompt'")
                })?;
                let res = tools::send_jules_message(session_id, prompt, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.request_verification
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.request_verification",
        "Ask Jules to verify results.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "testCommand": { "type": "string" },
                "testCwd": { "type": "string" },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["sessionId", "testCommand"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let test_command = args.get("testCommand").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'testCommand'")
                })?;
                let test_cwd = args.get("testCwd").and_then(|v| v.as_str()).map(|s| s.to_string());
                let timeout_ms = args.get("timeoutMs").and_then(|v| v.as_u64());

                let res = tools::request_jules_verification(session_id, test_command, test_cwd, timeout_ms, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.pending_actions
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.pending_actions",
        "List Jules sessions needing actions.",
        json!({
            "type": "object"
        }),
        move |_| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = tools::list_jules_pending_actions(&config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.collect_outputs
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.collect_outputs",
        "Collect Jules outputs.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" }
            },
            "required": ["sessionId"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let res = tools::collect_jules_outputs(session_id, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.watch
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.watch",
        "Poll Jules session state.",
        json!({
            "type": "object",
            "properties": {
                "sessionId": { "type": "string" },
                "states": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["sessionId", "states"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let session_id = args.get("sessionId").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'sessionId'")
                })?;
                let states_val = args.get("states").ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'states'")
                })?;
                let states: Vec<String> = serde_json::from_value(states_val.clone()).map_err(|e| {
                    AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid states parameter: {}", e))
                })?;
                let timeout_ms = args.get("timeoutMs").and_then(|v| v.as_u64());

                let res = tools::watch_jules_session(session_id, &states, timeout_ms, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.reconcile_local_runs
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.reconcile_local_runs",
        "Reconcile local Jules bookkeeping.",
        json!({
            "type": "object"
        }),
        move |_| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let res = tools::reconcile_local_jules_runs(&config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.remote_new
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.remote_new",
        "Deprecated shim for remote tasks.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "startingBranch": { "type": "string" },
                "argv": { "type": "array", "items": { "type": "string" } },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::JulesRemoteNewInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::jules_remote_new(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.remote_list
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.remote_list",
        "Deprecated shim for listing remote tasks.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::JulesRemoteListInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::jules_remote_list(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.remote_apply
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.remote_apply",
        "Deprecated shim for applying remote tasks.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "sessionId": { "type": "string" },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "sessionId"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::JulesRemoteApplyInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::jules_remote_apply(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // jules.api_create_session
    let c = config.clone();
    let s = store.clone();
    server.register_tool(
        "jules.api_create_session",
        "Deprecated compatibility shim for Jules API session creation.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "prompt": { "type": "string" },
                "source": { "type": "string" },
                "startingBranch": { "type": "string" },
                "title": { "type": "string" },
                "requirePlanApproval": { "type": "boolean" },
                "automationMode": { "type": "string" },
                "timeoutMs": { "type": "integer" }
            },
            "required": ["cwd", "prompt"]
        }),
        move |args| {
            let config = c.clone();
            let store = s.clone();
            async move {
                let input = serde_json::from_value::<tools::JulesCreateSessionInput>(args)
                    .map_err(|e| AgentCliError::new(ErrorCode::CommandBlocked, &format!("Invalid arguments: {}", e)))?;
                let res = tools::create_jules_session(input, &config, &store).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );

    // gh.pr_list
    let c = config.clone();
    server.register_tool(
        "gh.pr_list",
        "List open pull requests.",
        json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" }
            },
            "required": ["cwd"]
        }),
        move |args| {
            let config = c.clone();
            async move {
                let cwd = args.get("cwd").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentCliError::new(ErrorCode::CommandBlocked, "Missing required parameter 'cwd'")
                })?;
                let res = tools::gh_pr_list(cwd, &config).await?;
                Ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap() }] }))
            }
        },
    );
}
