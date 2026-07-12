mod config;
mod errors;
mod jules_client;
mod jules_status;
mod mcp;
mod policy;
mod redaction;
mod registration;
mod runner;
mod sanity;
mod store;
mod tools;

use crate::config::{load_config, Config};
use crate::errors::AgentCliError;
use crate::mcp::McpServer;
use crate::runner::SessionManager;
use crate::store::Store;
use serde::Serialize;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Arc::new(load_config());
    let store = Arc::new(Store::new(&config.state_dir)?);
    let session_manager = Arc::new(SessionManager::new());

    let instructions = concat!(
        "Multi-level executor bridge for GitHub Copilot CLI and Google Jules. ",
        "FIRST CALL after compaction or long waits: agent_cli.overview — it shows all running, ",
        "pending-input, done, and stale sessions plus executor health in one response. ",
        "BEFORE dispatching: call agent_cli.capabilities then agent_cli.executor_health. ",
        "LOCAL TARGET-REPO ACTIONS (shell, read, write, git-local, test, build) are broadly allowed ",
        "with orchestrator-agent approval — no user approval needed for these. ",
        "ORCHESTRATOR-APPROVAL actions (env files, remote API calls, branch pushes, broad Copilot modes, ",
        "Jules plan approvals): subagent proposes, orchestrator approves — user is NOT asked. ",
        "USER-APPROVAL or External Action Required: prod deploy, provider settings, billing, ",
        "credential rotation, destructive remote git, sudo/biometric-gated actions. ",
        "Never silently substitute Jules for Copilot or Copilot for Jules — record formal deviations ",
        "in .agent/workteam/executor-deviations.md. ",
        "Run agent_cli.sanity_check before reviewing executor output on sensitive repositories. ",
        "Use agent_cli.quarantine if executor output looks suspicious."
    );

    let server = McpServer::new("agent-cli-mcp-rust", "0.1.0", instructions);

    // ── Register Resources and Tools ──────────────────────────────────────────
    registration::register_all(
        &server,
        config.clone(),
        store.clone(),
        session_manager.clone(),
    );

    // Run the server loops asynchronously
    server.run().await?;

    Ok(())
}

// ── Overview Implementation ──

mod tools_overview {
    use super::*;
    use crate::jules_status::NormalizedJulesStatus;

    pub async fn get_overview(
        repo: Option<String>,
        include_done: bool,
        _include_logs: bool,
        config: &Config,
        store: &Store,
    ) -> Result<OverviewResult, AgentCliError> {
        // Implement get_overview parsing and formatting
        // (Similar to Overview.ts)
        let mut running = Vec::new();
        let mut awaiting_input = Vec::new();
        let mut done = Vec::new();
        let mut failed_or_stale = Vec::new();

        let list_args = tools::ListSessionsArgs {
            include_activities: Some(false),
            page_size: Some(50),
        };

        let j_sessions = tools::list_jules_sessions(list_args, config, store)
            .await
            .ok()
            .and_then(|v| v.get("sessions").cloned())
            .and_then(|v| serde_json::from_value::<Vec<tools::JulesStatusSummary>>(v).ok())
            .unwrap_or_default();

        for s in j_sessions {
            let status = s.normalized_status.clone();
            let entry = OverviewEntry {
                service: "jules".to_string(),
                id: s
                    .session_id
                    .clone()
                    .unwrap_or_else(|| s.run_id.clone().unwrap_or_default()),
                task_id: s.title.clone(),
                state: status.as_str().to_string(),
                last_activity: s.latest_activity_time.clone(),
                needs_action: s.needed_action.clone(),
                summary: None,
                reason: None,
                recommended_action: None,
            };

            match status {
                NormalizedJulesStatus::AwaitingPlanApproval => {
                    awaiting_input.push(entry);
                }
                NormalizedJulesStatus::AwaitingUserFeedback | NormalizedJulesStatus::Paused => {
                    awaiting_input.push(entry);
                }
                NormalizedJulesStatus::InProgress
                | NormalizedJulesStatus::Planning
                | NormalizedJulesStatus::Queued => {
                    running.push(entry);
                }
                NormalizedJulesStatus::Completed => {
                    if include_done {
                        done.push(entry);
                    }
                }
                NormalizedJulesStatus::Failed | NormalizedJulesStatus::StaleLocalRun => {
                    let mut e = entry;
                    e.reason = Some(status.as_str().to_string());
                    e.recommended_action =
                        Some(if status == NormalizedJulesStatus::StaleLocalRun {
                            "call jules.reconcile_local_runs".to_string()
                        } else {
                            "call jules.list_activities for diagnosis".to_string()
                        });
                    failed_or_stale.push(e);
                }
                _ => {}
            }
        }

        let local_runs = store.list("copilot", "all", Some(50));
        for run in local_runs {
            if let Some(ref r) = repo {
                if !run.cwd.starts_with(r) {
                    continue;
                }
            }

            let entry = OverviewEntry {
                service: "copilot".to_string(),
                id: run.id.clone(),
                task_id: None,
                state: run.status.clone(),
                last_activity: Some(run.started_at.clone()),
                needs_action: None,
                summary: Some(run.sanitized_command_summary.clone()),
                reason: None,
                recommended_action: None,
            };

            if run.status == "running" {
                running.push(entry);
            } else if run.status == "complete" || run.status == "killed" {
                if include_done {
                    done.push(entry);
                }
            } else if run.status == "failed" {
                let mut e = entry;
                e.reason = Some("process exited non-zero".to_string());
                e.recommended_action = Some("call agent_cli.read_output to diagnose".to_string());
                failed_or_stale.push(e);
            }
        }

        let caps = tools::discover_capabilities("all", config, false).await?;
        let copilot_health = caps
            .copilot
            .map(|c| {
                if c.installed {
                    "healthy"
                } else {
                    "unavailable"
                }
            })
            .unwrap_or("unavailable");
        let jules_health = caps
            .jules
            .map(|j| {
                if j.installed {
                    "healthy"
                } else {
                    "unavailable"
                }
            })
            .unwrap_or("unavailable");

        let health = ExecutorHealthSummary {
            copilot: copilot_health.to_string(),
            jules: jules_health.to_string(),
        };

        let recommended_next_action =
            derive_next_action(&running, &awaiting_input, &failed_or_stale, &health);
        let markdown = render_markdown(
            &running,
            &awaiting_input,
            &done,
            &failed_or_stale,
            &health,
            &recommended_next_action,
        );

        Ok(OverviewResult {
            running,
            awaiting_input,
            done,
            failed_or_stale,
            executor_health: health,
            recommended_next_action,
            markdown,
        })
    }

    #[derive(Debug, Serialize)]
    pub struct OverviewEntry {
        pub service: String,
        pub id: String,
        #[serde(rename = "taskId")]
        pub task_id: Option<String>,
        pub state: String,
        #[serde(rename = "lastActivity")]
        pub last_activity: Option<String>,
        #[serde(rename = "needsAction")]
        pub needs_action: Option<String>,
        pub summary: Option<String>,
        pub reason: Option<String>,
        #[serde(rename = "recommendedAction")]
        pub recommended_action: Option<String>,
    }

    #[derive(Debug, Serialize)]
    pub struct ExecutorHealthSummary {
        pub copilot: String,
        pub jules: String,
    }

    #[derive(Debug, Serialize)]
    pub struct OverviewResult {
        pub running: Vec<OverviewEntry>,
        #[serde(rename = "awaitingInput")]
        pub awaiting_input: Vec<OverviewEntry>,
        pub done: Vec<OverviewEntry>,
        #[serde(rename = "failedOrStale")]
        pub failed_or_stale: Vec<OverviewEntry>,
        #[serde(rename = "executorHealth")]
        pub executor_health: ExecutorHealthSummary,
        #[serde(rename = "recommendedNextAction")]
        pub recommended_next_action: String,
        pub markdown: String,
    }

    fn derive_next_action(
        running: &[OverviewEntry],
        awaiting_input: &[OverviewEntry],
        failed_or_stale: &[OverviewEntry],
        health: &ExecutorHealthSummary,
    ) -> String {
        if !awaiting_input.is_empty() {
            let first = &awaiting_input[0];
            if first.needs_action.as_deref() == Some("approve_plan") {
                return format!(
                    "Jules session {} is awaiting plan approval. Call jules.get_status to inspect the plan, then jules.approve_plan if it is within scope.",
                    first.id
                );
            }
            return format!(
                "{} session {} needs input ({:?}). Respond before dispatching new work.",
                first.service, first.id, first.needs_action
            );
        }
        if !failed_or_stale.is_empty() {
            return format!(
                "{} session(s) failed or are stale. Diagnose before dispatching new work.",
                failed_or_stale.len()
            );
        }
        if !running.is_empty() {
            return format!(
                "{} session(s) in progress. Poll with jules.get_status or agent_cli.read_output.",
                running.len()
            );
        }
        if health.copilot == "unavailable" && health.jules == "unavailable" {
            return "Both executors are unavailable. Check agent_cli.executor_health and fix before dispatching.".to_string();
        }
        "No active sessions. Ready to dispatch new work. Call agent_cli.capabilities and agent_cli.executor_health first.".to_string()
    }

    fn render_markdown(
        running: &[OverviewEntry],
        awaiting_input: &[OverviewEntry],
        done: &[OverviewEntry],
        failed_or_stale: &[OverviewEntry],
        health: &ExecutorHealthSummary,
        next_action: &str,
    ) -> String {
        let mut lines = vec!["# Agent CLI Overview".to_string(), "".to_string()];

        let fmt = |e: &OverviewEntry| {
            let mut s = format!("- **{}** `{}` state={}", e.service, e.id, e.state);
            if let Some(ref tid) = e.task_id {
                s.push_str(&format!(" task={}", tid));
            }
            if let Some(ref act) = e.last_activity {
                s.push_str(&format!(" lastActivity={}", act));
            }
            if let Some(ref na) = e.needs_action {
                s.push_str(&format!(" **needsAction={}**", na));
            }
            if let Some(ref sum) = e.summary {
                let limit = std::cmp::min(sum.len(), 80);
                s.push_str(&format!(" cmd={}", &sum[..limit]));
            }
            s
        };

        lines.push("## Running".to_string());
        if running.is_empty() {
            lines.push("_(none)_".to_string());
        } else {
            for e in running {
                lines.push(fmt(e));
            }
        }

        lines.push("".to_string());
        lines.push("## Awaiting Input".to_string());
        if awaiting_input.is_empty() {
            lines.push("_(none)_".to_string());
        } else {
            for e in awaiting_input {
                lines.push(fmt(e));
            }
        }

        if !done.is_empty() {
            lines.push("".to_string());
            lines.push("## Done".to_string());
            for e in done {
                lines.push(fmt(e));
            }
        }

        lines.push("".to_string());
        lines.push("## Failed / Stale".to_string());
        if failed_or_stale.is_empty() {
            lines.push("_(none)_".to_string());
        } else {
            for e in failed_or_stale {
                lines.push(format!(
                    "- **{}** `{}` reason={} → {}",
                    e.service,
                    e.id,
                    e.reason.as_deref().unwrap_or("unknown"),
                    e.recommended_action.as_deref().unwrap_or("investigate")
                ));
            }
        }

        lines.push("".to_string());
        lines.push("## Executor Health".to_string());
        lines.push(format!("- Copilot: **{}**", health.copilot));
        lines.push(format!("- Jules: **{}**", health.jules));

        lines.push("".to_string());
        lines.push("## Recommended Next Action".to_string());
        lines.push(next_action.to_string());

        lines.join("\n")
    }
}

// ── Expose get_overview ──
