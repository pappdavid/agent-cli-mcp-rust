use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{canonicalize_path, Config};
use crate::errors::{AgentCliError, ErrorCode};
use crate::jules_client::{CreateJulesSessionRequest, JulesClient};
use crate::jules_status::{
    build_jules_status, is_active_normalized_status, normalize_jules_session_state,
    BuildJulesStatusInput as JulesStatusInput, GithubRepoContext, NormalizedJulesStatus,
    SourceContext,
};

pub use crate::jules_status::JulesStatusSummary;
use crate::policy::{
    assert_is_worktree, assert_not_quarantined, detect_git_context, get_tool_preset,
    merge_tool_permissions, validate_argv, validate_cwd, validate_mode, validate_service,
};
use crate::redaction::redact_strict;
use crate::runner::{spawn_for_capability, spawn_process, SessionManager, SpawnOptions};
use crate::store::{generate_run_id, AgentRun, AgentRunPatch, Store};

// ── Capabilities Discovery ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotCapabilities {
    pub installed: bool,
    pub version: Option<String>,
    pub prompt_arg: Option<String>,
    pub interactive_prompt_arg: Option<String>,
    pub model_arg: Option<String>,
    pub autopilot_arg: Option<String>,
    pub allow_all_tools_arg: Option<String>,
    pub no_ask_user_arg: Option<String>,
    pub stream_arg: Option<String>,
    pub continue_arg: Option<String>,
    pub agent_arg: Option<String>,
    pub allow_tool_arg: Option<String>,
    pub deny_tool_arg: Option<String>,
    pub available_tools_arg: Option<String>,
    pub excluded_tools_arg: Option<String>,
    pub supports_model_flag: bool,
    pub supports_allow_all_tools: bool,
    pub supports_allow_tool: bool,
    pub supports_deny_tool: bool,
    pub supports_autopilot: bool,
    pub supports_continue: bool,
    pub supports_custom_agents: bool,
    pub supports_fleet_slash_command: bool,
    pub supports_prompt_file: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesCapabilities {
    pub installed: bool,
    pub version: Option<String>,
    pub version_command: Option<String>,
    pub help_command: Option<String>,
    pub supports_remote_new: bool,
    pub supports_remote_list: bool,
    pub supports_remote_apply: bool,
    pub remote_new_session_flag: Option<String>,
    pub remote_list_session_flag: Option<String>,
    pub remote_pull_command: Option<String>,
    pub supports_api: bool,
    pub api_configured: bool,
    pub supports_explicit_model_flag: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorCapabilities {
    pub installed: bool,
    pub version: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copilot: Option<CopilotCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jules: Option<JulesCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ExecutorCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<ExecutorCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<ExecutorCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<ExecutorCapabilities>,
    pub probe_timestamp: String,
}

struct CapabilityCache {
    result: CapabilityResult,
    timestamp: Instant,
}

static CAPABILITY_CACHE: Mutex<Option<CapabilityCache>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// Resolve the configured binary path for a given service name.
pub fn resolve_binary<'a>(service: &str, config: &'a Config) -> &'a str {
    match service {
        "copilot" => &config.copilot_bin,
        "jules" => &config.jules_bin,
        "gemini" => &config.gemini_bin,
        "codex" => &config.codex_bin,
        "opencode" => &config.opencode_bin,
        "claude" => &config.claude_bin,
        "gh" => &config.gh_bin,
        _ => "unknown",
    }
}

async fn probe_generic_executor(bin: &str) -> ExecutorCapabilities {
    let ver_res = spawn_for_capability(bin, &[String::from("--version")], 10000).await;
    let (installed, version_output) = match ver_res {
        Ok((success, output)) => (success || !output.is_empty(), Some(output)),
        Err(_) => (false, None),
    };

    if !installed {
        return ExecutorCapabilities {
            installed: false,
            version: None,
            notes: vec![format!("Binary not found or not executable: {}", bin)],
        };
    }

    let version = version_output
        .map(|o| o.lines().next().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty());

    ExecutorCapabilities {
        installed: true,
        version,
        notes: vec![],
    }
}

pub async fn discover_capabilities(
    service: &str,
    config: &Config,
    refresh: bool,
) -> Result<CapabilityResult, AgentCliError> {
    if !refresh {
        let cache = CAPABILITY_CACHE.lock().unwrap();
        if let Some(ref c) = *cache {
            if c.timestamp.elapsed() < CACHE_TTL {
                if service == "all" {
                    return Ok(c.result.clone());
                }
                // Return single-service filtered view from cache
                let mut filtered = CapabilityResult {
                    copilot: None,
                    jules: None,
                    gemini: None,
                    codex: None,
                    opencode: None,
                    claude: None,
                    probe_timestamp: c.result.probe_timestamp.clone(),
                };
                match service {
                    "copilot" if c.result.copilot.is_some() => {
                        filtered.copilot = c.result.copilot.clone();
                        return Ok(filtered);
                    }
                    "jules" if c.result.jules.is_some() => {
                        filtered.jules = c.result.jules.clone();
                        return Ok(filtered);
                    }
                    "gemini" if c.result.gemini.is_some() => {
                        filtered.gemini = c.result.gemini.clone();
                        return Ok(filtered);
                    }
                    "codex" if c.result.codex.is_some() => {
                        filtered.codex = c.result.codex.clone();
                        return Ok(filtered);
                    }
                    "opencode" if c.result.opencode.is_some() => {
                        filtered.opencode = c.result.opencode.clone();
                        return Ok(filtered);
                    }
                    "claude" if c.result.claude.is_some() => {
                        filtered.claude = c.result.claude.clone();
                        return Ok(filtered);
                    }
                    _ => {} // Cache miss for this service, fall through to probe
                }
            }
        }
    }

    let mut result = CapabilityResult {
        copilot: None,
        jules: None,
        gemini: None,
        codex: None,
        opencode: None,
        claude: None,
        probe_timestamp: Utc::now().to_rfc3339(),
    };

    if service == "copilot" || service == "all" {
        result.copilot = Some(probe_copilot(&config.copilot_bin).await);
    }
    if service == "jules" || service == "all" {
        result.jules = Some(probe_jules(&config.jules_bin, config).await);
    }
    if service == "gemini" || service == "all" {
        result.gemini = Some(probe_generic_executor(&config.gemini_bin).await);
    }
    if service == "codex" || service == "all" {
        result.codex = Some(probe_generic_executor(&config.codex_bin).await);
    }
    if service == "opencode" || service == "all" {
        result.opencode = Some(probe_generic_executor(&config.opencode_bin).await);
    }
    if service == "claude" || service == "all" {
        result.claude = Some(probe_generic_executor(&config.claude_bin).await);
    }

    let mut cache = CAPABILITY_CACHE.lock().unwrap();
    let updated_result = if let Some(ref mut c) = *cache {
        match service {
            "copilot" => c.result.copilot = result.copilot.clone(),
            "jules" => c.result.jules = result.jules.clone(),
            "gemini" => c.result.gemini = result.gemini.clone(),
            "codex" => c.result.codex = result.codex.clone(),
            "opencode" => c.result.opencode = result.opencode.clone(),
            "claude" => c.result.claude = result.claude.clone(),
            _ => c.result = result.clone(), // "all"
        }
        c.timestamp = Instant::now();
        c.result.clone()
    } else {
        *cache = Some(CapabilityCache {
            result: result.clone(),
            timestamp: Instant::now(),
        });
        result.clone()
    };

    Ok(updated_result)
}

async fn probe_copilot(bin: &str) -> CopilotCapabilities {
    let mut notes = Vec::new();

    let ver_res = spawn_for_capability(bin, &[String::from("--version")], 10000).await;
    let installed = match ver_res {
        Ok((success, output)) => success || !output.is_empty(),
        Err(_) => false,
    };

    if !installed {
        return CopilotCapabilities {
            installed: false,
            version: None,
            prompt_arg: None,
            interactive_prompt_arg: None,
            model_arg: None,
            autopilot_arg: None,
            allow_all_tools_arg: None,
            no_ask_user_arg: None,
            stream_arg: None,
            continue_arg: None,
            agent_arg: None,
            allow_tool_arg: None,
            deny_tool_arg: None,
            available_tools_arg: None,
            excluded_tools_arg: None,
            supports_model_flag: false,
            supports_allow_all_tools: false,
            supports_allow_tool: false,
            supports_deny_tool: false,
            supports_autopilot: false,
            supports_continue: false,
            supports_custom_agents: false,
            supports_fleet_slash_command: false,
            supports_prompt_file: false,
            notes: vec![format!("Binary not found or not executable: {}", bin)],
        };
    }

    let version = spawn_for_capability(bin, &[String::from("--version")], 10000)
        .await
        .ok()
        .map(|(_, o)| o.lines().next().unwrap_or("").trim().to_string());

    let help_out = spawn_for_capability(bin, &[String::from("--help")], 10000)
        .await
        .ok()
        .map(|(_, o)| o)
        .unwrap_or_default();

    let help_full_out = spawn_for_capability(bin, &[String::from("help")], 10000)
        .await
        .ok()
        .map(|(_, o)| o)
        .unwrap_or_default();

    let combined = format!("{} {}", help_out, help_full_out).to_lowercase();

    let supports_model_flag = combined.contains("--model");
    let supports_allow_all_tools = combined.contains("--allow-all-tools");
    let supports_allow_tool = combined.contains("--allow-tool");
    let supports_deny_tool = combined.contains("--deny-tool");
    let supports_available_tools = combined.contains("--available-tools");
    let supports_excluded_tools = combined.contains("--excluded-tools");
    let supports_autopilot = combined.contains("--autopilot");
    let supports_continue = combined.contains("--continue");
    let supports_custom_agents = combined.contains("--agent");
    let supports_no_ask_user = combined.contains("--no-ask-user");
    let supports_stream = combined.contains("--stream");
    let supports_interactive_prompt = combined.contains("-i") || combined.contains("--interactive");
    let supports_fleet_slash_command =
        combined.contains("/fleet") || combined.contains("fleet mode");
    let supports_prompt_file = combined.contains("--prompt-file");

    if !supports_model_flag {
        notes.push(
            "--model flag not detected in help; model selection may be unsupported".to_string(),
        );
    }
    if !supports_allow_all_tools {
        notes.push(
            "--allow-all-tools not detected; non-interactive mode may require explicit --allow-tool lists"
                .to_string(),
        );
    }
    if !supports_allow_tool {
        notes.push("--allow-tool not detected; tool allow lists may be unsupported".to_string());
    }
    if !supports_deny_tool {
        notes.push("--deny-tool not detected; tool deny lists may be unsupported".to_string());
    }
    if !supports_fleet_slash_command {
        notes.push(
            "/fleet slash command not confirmed; do not inject /fleet automatically".to_string(),
        );
    }

    CopilotCapabilities {
        installed: true,
        version,
        prompt_arg: Some("-p".to_string()),
        interactive_prompt_arg: if supports_interactive_prompt {
            Some("-i".to_string())
        } else {
            None
        },
        model_arg: if supports_model_flag {
            Some("--model".to_string())
        } else {
            None
        },
        allow_all_tools_arg: if supports_allow_all_tools {
            Some("--allow-all-tools".to_string())
        } else {
            None
        },
        autopilot_arg: if supports_autopilot {
            Some("--autopilot".to_string())
        } else {
            None
        },
        no_ask_user_arg: if supports_no_ask_user {
            Some("--no-ask-user".to_string())
        } else {
            None
        },
        stream_arg: if supports_stream {
            Some("--stream".to_string())
        } else {
            None
        },
        continue_arg: if supports_continue {
            Some("--continue".to_string())
        } else {
            None
        },
        agent_arg: if supports_custom_agents {
            Some("--agent".to_string())
        } else {
            None
        },
        allow_tool_arg: if supports_allow_tool {
            Some("--allow-tool".to_string())
        } else {
            None
        },
        deny_tool_arg: if supports_deny_tool {
            Some("--deny-tool".to_string())
        } else {
            None
        },
        available_tools_arg: if supports_available_tools {
            Some("--available-tools".to_string())
        } else {
            None
        },
        excluded_tools_arg: if supports_excluded_tools {
            Some("--excluded-tools".to_string())
        } else {
            None
        },
        supports_model_flag,
        supports_allow_all_tools,
        supports_allow_tool,
        supports_deny_tool,
        supports_autopilot,
        supports_continue,
        supports_custom_agents,
        supports_fleet_slash_command,
        supports_prompt_file,
        notes,
    }
}

async fn probe_jules(bin: &str, config: &Config) -> JulesCapabilities {
    let mut notes = Vec::new();

    let help_res = spawn_for_capability(bin, &[String::from("help")], 10000)
        .await
        .ok();
    let installed = help_res
        .as_ref()
        .map(|(success, output)| *success || !output.is_empty())
        .unwrap_or(false);

    if !installed {
        return JulesCapabilities {
            installed: false,
            version: None,
            version_command: None,
            help_command: None,
            supports_remote_new: false,
            supports_remote_list: false,
            supports_remote_apply: false,
            remote_new_session_flag: None,
            remote_list_session_flag: None,
            remote_pull_command: None,
            supports_api: false,
            api_configured: false,
            supports_explicit_model_flag: false,
            notes: vec![format!("Binary not found or not executable: {}", bin)],
        };
    }

    let version = spawn_for_capability(bin, &[String::from("version")], 10000)
        .await
        .ok()
        .map(|(_, o)| o.lines().next().unwrap_or("").trim().to_string());

    let help_out = help_res.map(|(_, o)| o).unwrap_or_default();
    let remote_help = spawn_for_capability(
        bin,
        &[String::from("remote"), String::from("--help")],
        10000,
    )
    .await
    .ok()
    .map(|(_, o)| o)
    .unwrap_or_default();
    let remote_new_help = spawn_for_capability(
        bin,
        &[
            String::from("remote"),
            String::from("new"),
            String::from("--help"),
        ],
        10000,
    )
    .await
    .ok()
    .map(|(_, o)| o)
    .unwrap_or_default();
    let remote_list_help = spawn_for_capability(
        bin,
        &[
            String::from("remote"),
            String::from("list"),
            String::from("--help"),
        ],
        10000,
    )
    .await
    .ok()
    .map(|(_, o)| o)
    .unwrap_or_default();
    let remote_pull_help = spawn_for_capability(
        bin,
        &[
            String::from("remote"),
            String::from("pull"),
            String::from("--help"),
        ],
        10000,
    )
    .await
    .ok()
    .map(|(_, o)| o)
    .unwrap_or_default();

    let all_output = format!(
        "{} {} {} {} {}",
        help_out, remote_help, remote_new_help, remote_list_help, remote_pull_help
    )
    .to_lowercase();

    let supports_remote_new =
        all_output.contains("remote new") || all_output.contains("remote\tnew");
    let supports_remote_list =
        all_output.contains("remote list") || all_output.contains("remote\tlist");
    let supports_remote_apply =
        all_output.contains("remote pull") || all_output.contains("remote\tpull");
    let supports_explicit_model_flag = all_output.contains("--model");

    if !supports_explicit_model_flag {
        notes.push("Explicit model selection not verified; only 'default' and 'auto' accepted until --model is confirmed".to_string());
    }

    let api_configured = config.jules_api_key.is_some() || config.jules_api_key_cmd_parts.is_some();
    if config.jules_api_key.is_some() {
        notes.push("Jules API key configured via JULES_API_KEY".to_string());
    } else if config.jules_api_key_cmd_parts.is_some() {
        notes.push("Jules API key command configured via JULES_API_KEY_CMD".to_string());
    }

    JulesCapabilities {
        installed: true,
        version,
        version_command: Some("jules version".to_string()),
        help_command: Some("jules help".to_string()),
        supports_remote_new,
        supports_remote_list,
        supports_remote_apply,
        remote_new_session_flag: if remote_new_help.contains("--session") {
            Some("--session".to_string())
        } else {
            None
        },
        remote_list_session_flag: if remote_list_help.contains("--session") {
            Some("--session".to_string())
        } else {
            None
        },
        remote_pull_command: if supports_remote_apply {
            Some("remote pull --session <id> --apply".to_string())
        } else {
            None
        },
        supports_api: api_configured,
        api_configured,
        supports_explicit_model_flag,
        notes,
    }
}

// ── Health Checking ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ExecutorHealthResult {
    pub service: String,
    pub status: String, // "healthy" | "degraded" | "unavailable"
    #[serde(rename = "affectedCapabilities")]
    pub affected_capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastFailure")]
    pub last_failure: Option<String>,
    #[serde(rename = "safeProfilesAvailable")]
    pub safe_profiles_available: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recommendedFallback")]
    pub recommended_fallback: Option<String>,
}

pub async fn executor_health(
    service: &str,
    config: &Config,
) -> Result<serde_json::Value, AgentCliError> {
    let caps = discover_capabilities(service, config, true).await?;
    let mut results = Vec::new();

    if service == "copilot" || service == "all" {
        let cop = caps.copilot;
        match cop {
            None => {}
            Some(c) => {
                if !c.installed {
                    results.push(ExecutorHealthResult {
                        service: "copilot".to_string(),
                        status: "unavailable".to_string(),
                        affected_capabilities: vec![
                            "file-edit".to_string(),
                            "fleet".to_string(),
                            "prompt".to_string(),
                            "autopilot".to_string(),
                            "deny-tool-profiles".to_string(),
                        ],
                        reason: c.notes.first().cloned(),
                        last_failure: None,
                        safe_profiles_available: vec![],
                        recommended_fallback: Some("jules".to_string()),
                    });
                } else {
                    let mut affected = Vec::new();
                    if !c.supports_deny_tool {
                        affected.push("deny-tool-profiles".to_string());
                    }
                    if !c.supports_allow_tool {
                        affected.push("allow-tool-profiles".to_string());
                    }
                    if !c.supports_autopilot {
                        affected.push("autopilot".to_string());
                    }
                    if !c.supports_fleet_slash_command {
                        affected.push("fleet".to_string());
                    }

                    let safe_profiles = vec![
                        "copilot-file-edit".to_string(),
                        "copilot-safe-dev".to_string(),
                        "copilot-expanded-worktree".to_string(),
                        "copilot-target-repo".to_string(),
                    ];

                    results.push(ExecutorHealthResult {
                        service: "copilot".to_string(),
                        status: if affected.is_empty() {
                            "healthy".to_string()
                        } else {
                            "degraded".to_string()
                        },
                        affected_capabilities: affected.clone(),
                        reason: if !affected.is_empty() {
                            Some(format!(
                                "Detected missing Copilot CLI capabilities: {}",
                                affected.join(", ")
                            ))
                        } else {
                            None
                        },
                        last_failure: None,
                        safe_profiles_available: safe_profiles,
                        recommended_fallback: if !affected.is_empty() {
                            Some("jules".to_string())
                        } else {
                            None
                        },
                    });
                }
            }
        }
    }

    if service == "jules" || service == "all" {
        let jul = caps.jules;
        match jul {
            None => {}
            Some(j) => {
                if !j.installed {
                    results.push(ExecutorHealthResult {
                        service: "jules".to_string(),
                        status: "unavailable".to_string(),
                        affected_capabilities: vec![
                            "remote-session".to_string(),
                            "plan-approval".to_string(),
                            "verification".to_string(),
                            "api".to_string(),
                        ],
                        reason: j.notes.first().cloned(),
                        last_failure: None,
                        safe_profiles_available: vec![],
                        recommended_fallback: Some("copilot".to_string()),
                    });
                } else {
                    let mut affected = Vec::new();
                    if !j.supports_remote_new {
                        affected.push("remote-session".to_string());
                    }
                    if !j.supports_remote_apply {
                        affected.push("remote-apply".to_string());
                    }
                    if !j.api_configured {
                        affected.push("api".to_string());
                    }

                    results.push(ExecutorHealthResult {
                        service: "jules".to_string(),
                        status: if affected.contains(&"remote-session".to_string()) {
                            "unavailable".to_string()
                        } else if !affected.is_empty() {
                            "degraded".to_string()
                        } else {
                            "healthy".to_string()
                        },
                        affected_capabilities: affected.clone(),
                        reason: if !affected.is_empty() {
                            Some(format!(
                                "Detected missing Jules capabilities: {}",
                                affected.join(", ")
                            ))
                        } else {
                            None
                        },
                        last_failure: None,
                        safe_profiles_available: if j.api_configured {
                            vec!["api".to_string(), "cli".to_string()]
                        } else if j.supports_remote_new {
                            vec!["cli".to_string()]
                        } else {
                            vec![]
                        },
                        recommended_fallback: if affected.contains(&"remote-session".to_string()) {
                            Some("copilot".to_string())
                        } else {
                            None
                        },
                    });
                }
            }
        }
    }

    // Generic executor health for Gemini, Codex, OpenCode, Claude
    for (svc_name, cap_opt) in [
        ("gemini", caps.gemini),
        ("codex", caps.codex),
        ("opencode", caps.opencode),
        ("claude", caps.claude),
    ] {
        if service != svc_name && service != "all" {
            continue;
        }
        if let Some(exec_cap) = cap_opt {
            if !exec_cap.installed {
                results.push(ExecutorHealthResult {
                    service: svc_name.to_string(),
                    status: "unavailable".to_string(),
                    affected_capabilities: vec!["prompt".to_string(), "file-edit".to_string()],
                    reason: exec_cap.notes.first().cloned(),
                    last_failure: None,
                    safe_profiles_available: vec![],
                    recommended_fallback: Some("copilot".to_string()),
                });
            } else {
                results.push(ExecutorHealthResult {
                    service: svc_name.to_string(),
                    status: "healthy".to_string(),
                    affected_capabilities: vec![],
                    reason: None,
                    last_failure: None,
                    safe_profiles_available: vec!["cli".to_string()],
                    recommended_fallback: None,
                });
            }
        }
    }

    Ok(json!({ "results": results }))
}

// ── Profile Smoke Testing ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ProfileCheck {
    pub name: String,
    pub passed: bool,
    pub expected: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfileSmokeTestResult {
    pub service: String,
    pub profile: String,
    pub passed: bool,
    pub checks: Vec<ProfileCheck>,
}

pub async fn test_executor_profile(
    service: &str,
    profile: &str,
    config: &Config,
) -> Result<serde_json::Value, AgentCliError> {
    let preset = match get_tool_preset(profile) {
        Some(p) => p,
        None => {
            return Ok(json!({
                "service": service,
                "profile": profile,
                "passed": false,
                "checks": [{
                    "name": "profile-exists",
                    "passed": false,
                    "expected": format!("Profile \"{}\" defined in TOOL_PRESETS", profile),
                    "details": "Available profiles: copilot-file-edit, copilot-safe-dev, copilot-expanded-worktree, copilot-target-repo"
                }]
            }));
        }
    };

    let (_, effective_deny) =
        merge_tool_permissions(&preset.allow_tools, &preset.deny_tools, false);
    let mut checks = Vec::new();

    // Allows normal file reads check
    let overbroad: Vec<String> = effective_deny
        .iter()
        .filter(|d| {
            d.as_str() == "read"
                || d.as_str() == "read(*)"
                || (d.starts_with("read(")
                    && !d.starts_with("read(.env")
                    && !d.starts_with("read(.git"))
        })
        .cloned()
        .collect();

    checks.push(ProfileCheck {
        name: "allows-normal-file-reads".to_string(),
        passed: overbroad.is_empty(),
        expected: "No overbroad read() deny pattern blocks normal source files".to_string(),
        details: if !overbroad.is_empty() {
            Some(format!(
                "These deny patterns over-block file reads: {} — use MCP-side redaction + worktree isolation instead",
                overbroad.join(", ")
            ))
        } else {
            None
        },
    });

    // Env writes check
    let env_write_denied = effective_deny
        .iter()
        .any(|d| d == "write(.env*)" || d == "write(.env)");
    checks.push(ProfileCheck {
        name: "denies-env-writes".to_string(),
        passed: env_write_denied,
        expected: "write(.env*) in effective deny list".to_string(),
        details: if !env_write_denied {
            Some(
                "Env file writes not blocked — unsafe for any delegated file-edit task".to_string(),
            )
        } else {
            None
        },
    });

    // Memory write check
    let memory_denied = effective_deny.iter().any(|d| d == "memory");
    checks.push(ProfileCheck {
        name: "denies-memory-writes".to_string(),
        passed: memory_denied,
        expected: "memory in effective deny list".to_string(),
        details: if !memory_denied {
            Some(
                "Memory writes not blocked — MANDATORY_DENY_OVERLAY may not be applied".to_string(),
            )
        } else {
            None
        },
    });

    // Vercel/Supabase checks
    let vercel_prod_denied = effective_deny
        .iter()
        .any(|d| d.contains("vercel deploy --prod") || d.contains("vercel env:"));
    checks.push(ProfileCheck {
        name: "denies-provider-mutations".to_string(),
        passed: vercel_prod_denied,
        expected: "vercel deploy --prod and vercel env:* in effective deny list".to_string(),
        details: if !vercel_prod_denied {
            Some("Provider mutation commands not blocked".to_string())
        } else {
            None
        },
    });

    // Force push check
    let force_push_denied = effective_deny
        .iter()
        .any(|d| d.contains("git push --force"));
    checks.push(ProfileCheck {
        name: "denies-force-push".to_string(),
        passed: force_push_denied,
        expected: "shell(git push --force) in effective deny list".to_string(),
        details: if !force_push_denied {
            Some("Force push not blocked".to_string())
        } else {
            None
        },
    });

    if service == "copilot" {
        let caps = discover_capabilities("copilot", config, false).await?;
        let supports_deny = caps.copilot.is_some_and(|c| c.supports_deny_tool);
        checks.push(ProfileCheck {
            name: "copilot-supports-deny-tool-flag".to_string(),
            passed: supports_deny,
            expected: "--deny-tool flag supported by installed Copilot CLI".to_string(),
            details: if !supports_deny {
                Some("Copilot CLI does not support --deny-tool — deny profiles cannot be enforced at runtime; report MCP policy bug, do not silently widen permissions".to_string())
            } else {
                None
            },
        });
    }

    let passed = checks.iter().all(|c| c.passed);

    Ok(json!({
        "service": service,
        "profile": profile,
        "passed": passed,
        "checks": checks
    }))
}

// ── Spawning, Running, Backgrounding ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInput {
    pub service: String,
    pub mode: String,
    pub cwd: String,
    pub prompt: Option<String>,
    pub argv: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub allow_env_writes: Option<bool>,
    pub dry_run: Option<bool>,
}

pub async fn execute_run(
    args: RunInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    validate_service(&args.service)?;
    validate_mode(&args.mode)?;
    let validated_cwd = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&validated_cwd)?;

    let mut binary_args = Vec::new();

    // Mode specific base args
    if args.service == "copilot" {
        binary_args.push(args.mode.clone());
        if let Some(ref p) = args.prompt {
            binary_args.push("-p".to_string());
            binary_args.push(p.clone());
        }
    } else if args.service == "jules" {
        // Shims for jules remote new / apply / list shims
        binary_args.push(args.mode.clone());
    }

    if let Some(ref extra_argv) = args.argv {
        validate_argv(extra_argv)?;
        for a in extra_argv {
            binary_args.push(a.clone());
        }
    }

    // Standard safety deny list
    let (_merged_allow, merged_deny) = merge_tool_permissions(
        args.allow_tools.as_deref().unwrap_or(&[]),
        args.deny_tools.as_deref().unwrap_or(&[]),
        args.allow_env_writes.unwrap_or(false),
    );

    if args.service == "copilot" {
        let caps = discover_capabilities("copilot", config, false).await?;
        let supports_deny = caps.copilot.is_some_and(|c| c.supports_deny_tool);

        if supports_deny {
            for d in &merged_deny {
                binary_args.push("--deny-tool".to_string());
                binary_args.push(d.clone());
            }
        }
    }

    let bin = resolve_binary(&args.service, config);

    if args.dry_run.unwrap_or(false) {
        return Ok(json!({
            "ok": true,
            "dryRun": true,
            "binary": bin,
            "args": binary_args,
        }));
    }

    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).map_err(|e| {
        AgentCliError::new(
            ErrorCode::BackendFailed,
            &format!("Failed to create log directories: {}", e),
        )
    })?;

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let run = AgentRun {
        id: run_id.clone(),
        service: args.service.clone(),
        mode: args.mode.clone(),
        model: None,
        agent: None,
        cwd: validated_cwd.clone(),
        argv: binary_args.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: None,
        remote_source: None,
        remote_url: None,
        remote_state: None,
        normalized_status: None,
        last_activity_time: None,
        last_reconciled_at: None,
        stale_reason: None,
        sanitized_command_summary: format!("{} {}", bin, binary_args.join(" ")),
    };

    store.save(&run).map_err(|e| {
        AgentCliError::new(
            ErrorCode::BackendFailed,
            &format!("Failed to write state store: {}", e),
        )
    })?;

    let spawn_opts = SpawnOptions {
        cwd: validated_cwd,
        env,
        timeout_ms: Some(args.timeout_ms.unwrap_or(config.default_timeout_ms)),
        stdout_log_path: run.stdout_log.clone(),
        stderr_log_path: run.stderr_log.clone(),
        stdin_data: None,
    };

    let start_time = Instant::now();
    let result = spawn_process(bin, &binary_args, spawn_opts).await;

    let ended_at = Utc::now().to_rfc3339();

    match result {
        Ok(res) => {
            let status_str = if res.exit_code == Some(0) {
                "complete"
            } else {
                "failed"
            };
            store
                .update(
                    &run_id,
                    AgentRunPatch {
                        status: Some(status_str.to_string()),
                        ended_at: Some(ended_at.clone()),
                        exit_code: Some(res.exit_code),
                        ..Default::default()
                    },
                )
                .unwrap();

            Ok(json!({
                "ok": res.exit_code == Some(0),
                "runId": run_id,
                "exitCode": res.exit_code,
                "stdout": redact_strict(&res.stdout),
                "stderr": redact_strict(&res.stderr),
                "durationMs": start_time.elapsed().as_millis(),
            }))
        }
        Err(e) => {
            store
                .update(
                    &run_id,
                    AgentRunPatch {
                        status: Some("failed".to_string()),
                        ended_at: Some(ended_at),
                        exit_code: Some(None),
                        stale_reason: Some(format!("{}", e)),
                        ..Default::default()
                    },
                )
                .unwrap();
            Err(e)
        }
    }
}

pub async fn start_run(
    args: RunInput,
    config: &Config,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    validate_service(&args.service)?;
    validate_mode(&args.mode)?;
    let validated_cwd = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&validated_cwd)?;

    let mut binary_args = Vec::new();

    if args.service == "copilot" {
        binary_args.push(args.mode.clone());
        if let Some(ref p) = args.prompt {
            binary_args.push("-p".to_string());
            binary_args.push(p.clone());
        }
    } else {
        binary_args.push(args.mode.clone());
    }

    if let Some(ref extra_argv) = args.argv {
        validate_argv(extra_argv)?;
        for a in extra_argv {
            binary_args.push(a.clone());
        }
    }

    let (_merged_allow, merged_deny) = merge_tool_permissions(
        args.allow_tools.as_deref().unwrap_or(&[]),
        args.deny_tools.as_deref().unwrap_or(&[]),
        args.allow_env_writes.unwrap_or(false),
    );

    if args.service == "copilot" {
        let caps = discover_capabilities("copilot", config, false).await?;
        if caps.copilot.is_some_and(|c| c.supports_deny_tool) {
            for d in &merged_deny {
                binary_args.push("--deny-tool".to_string());
                binary_args.push(d.clone());
            }
        }
    }

    let bin = resolve_binary(&args.service, config);

    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).map_err(|e| {
        AgentCliError::new(
            ErrorCode::BackendFailed,
            &format!("Failed to allocate log paths: {}", e),
        )
    })?;

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let run = AgentRun {
        id: run_id.clone(),
        service: args.service.clone(),
        mode: args.mode.clone(),
        model: None,
        agent: None,
        cwd: validated_cwd.clone(),
        argv: binary_args.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: None,
        remote_source: None,
        remote_url: None,
        remote_state: None,
        normalized_status: None,
        last_activity_time: None,
        last_reconciled_at: None,
        stale_reason: None,
        sanitized_command_summary: format!("{} {}", bin, binary_args.join(" ")),
    };

    store.save(&run).unwrap();

    let spawn_opts = SpawnOptions {
        cwd: validated_cwd,
        env,
        timeout_ms: None,
        stdout_log_path: run.stdout_log.clone(),
        stderr_log_path: run.stderr_log.clone(),
        stdin_data: None,
    };

    let session_id = session_manager
        .start(
            bin,
            &binary_args,
            spawn_opts,
            Some(run_id.clone()),
            &args.service,
        )
        .await?;

    let _store_clone = Arc::new(Mutex::new(store.all())); // For callback monitoring
    let run_id_clone = run_id.clone();
    let session_manager_clone = session_manager.clone();
    let store_file_path = store.runs_file.clone();

    let session_id_clone = session_id.clone();
    // Spawn async monitor task to reconcile runs store when process ends
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Some(info) = session_manager_clone.get_session_info(&session_id_clone) {
                if info.status != "running" {
                    let store_inst = Store::new(store_file_path.parent().unwrap()).unwrap();
                    let patch = AgentRunPatch {
                        status: Some(info.status.clone()),
                        ended_at: Some(Utc::now().to_rfc3339()),
                        ..Default::default()
                    };
                    let _ = store_inst.update(&run_id_clone, patch);
                    break;
                }
            } else {
                break;
            }
        }
    });

    Ok(json!({
        "ok": true,
        "runId": run_id,
        "sessionId": session_id,
        "status": "running"
    }))
}

// ── Interactive Sessions ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSessionInput {
    pub service: String,
    pub cwd: String,
    pub argv: Vec<String>,
}

pub async fn start_session(
    args: StartSessionInput,
    config: &Config,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    validate_service(&args.service)?;
    let validated_cwd = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&validated_cwd)?;

    validate_argv(&args.argv)?;

    let bin = resolve_binary(&args.service, config);
    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).unwrap();

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let run = AgentRun {
        id: run_id.clone(),
        service: args.service.clone(),
        mode: "interactive".to_string(),
        model: None,
        agent: None,
        cwd: validated_cwd.clone(),
        argv: args.argv.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: None,
        remote_source: None,
        remote_url: None,
        remote_state: None,
        normalized_status: None,
        last_activity_time: None,
        last_reconciled_at: None,
        stale_reason: None,
        sanitized_command_summary: format!("{} {}", bin, args.argv.join(" ")),
    };
    store.save(&run).unwrap();

    let spawn_opts = SpawnOptions {
        cwd: validated_cwd,
        env,
        timeout_ms: None,
        stdout_log_path: run.stdout_log.clone(),
        stderr_log_path: run.stderr_log.clone(),
        stdin_data: None,
    };

    let session_id = session_manager
        .start(
            bin,
            &args.argv,
            spawn_opts,
            Some(run_id.clone()),
            &args.service,
        )
        .await?;

    Ok(json!({
        "ok": true,
        "sessionId": session_id,
        "runId": run_id,
        "status": "running"
    }))
}

// ── Git Worktree Management ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeInput {
    pub repo_path: String,
    pub task_id: String,
    pub base_branch: Option<String>,
    pub branch_name: Option<String>,
}

pub async fn create_worktree(
    args: CreateWorktreeInput,
    config: &Config,
) -> Result<serde_json::Value, AgentCliError> {
    let resolved_repo = validate_cwd(&args.repo_path, config)?;

    if !Path::new(&resolved_repo).join(".git").exists() {
        return Err(AgentCliError::new(
            ErrorCode::BackendFailed,
            &format!("Not a git repository: {}", resolved_repo),
        ));
    }

    let git_context = detect_git_context(&resolved_repo).ok_or_else(|| {
        AgentCliError::new(ErrorCode::BackendFailed, "Failed to resolve git context")
    })?;

    let worktree_parent = match config.worktree_root {
        Some(ref wr) => canonicalize_path(wr),
        None => {
            let parent = Path::new(&git_context.repo_root).parent().unwrap();
            parent.join("worktrees").to_string_lossy().to_string()
        }
    };

    fs::create_dir_all(&worktree_parent).map_err(|e| {
        AgentCliError::new(
            ErrorCode::BackendFailed,
            &format!("Failed to create worktree root: {}", e),
        )
    })?;

    // Sanitize task name
    let safe_task_id = args
        .task_id
        .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "_");
    let ts_36 = Utc::now().timestamp_millis().to_string();
    let branch_name = args
        .branch_name
        .clone()
        .unwrap_or_else(|| format!("task/{}-{}", safe_task_id, ts_36));
    let worktree_path = Path::new(&worktree_parent)
        .join(format!("{}-{}", safe_task_id, ts_36))
        .to_string_lossy()
        .to_string();

    let mut git_args = vec![
        "worktree".to_string(),
        "add".to_string(),
        worktree_path.clone(),
        "-b".to_string(),
        branch_name.clone(),
    ];
    if let Some(ref base) = args.base_branch {
        git_args.push(base.clone());
    }

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let log_dir = Path::new(&config.state_dir).join("worktree-logs");
    fs::create_dir_all(&log_dir).unwrap();
    let stdout_log = log_dir
        .join(format!("{}-stdout.log", safe_task_id))
        .to_string_lossy()
        .to_string();
    let stderr_log = log_dir
        .join(format!("{}-stderr.log", safe_task_id))
        .to_string_lossy()
        .to_string();

    let spawn_opts = SpawnOptions {
        cwd: git_context.repo_root.clone(),
        env,
        timeout_ms: Some(30000),
        stdout_log_path: stdout_log,
        stderr_log_path: stderr_log,
        stdin_data: None,
    };

    let result = spawn_process("git", &git_args, spawn_opts).await?;

    if result.exit_code != Some(0) {
        return Err(AgentCliError::new(
            ErrorCode::BackendFailed,
            &format!(
                "git worktree add failed (exit {:?}): {}",
                result.exit_code, result.stderr
            ),
        ));
    }

    Ok(json!({
        "ok": true,
        "worktreePath": worktree_path,
        "branchName": branch_name,
        "repoRoot": git_context.repo_root,
    }))
}

// ── Directory Quarantine ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineInput {
    pub cwd: String,
    pub run_id: Option<String>,
    pub reason: String,
}

pub async fn quarantine_directory(
    args: QuarantineInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let resolved_cwd = validate_cwd(&args.cwd, config)?;

    let marker_path = Path::new(&resolved_cwd).join(".agent-cli-quarantine");
    let report_path = Path::new(&resolved_cwd).join("QUARANTINE_REPORT.md");

    let marker_val = json!({
        "quarantinedAt": Utc::now().to_rfc3339(),
        "reason": args.reason,
        "runId": args.run_id,
        "clearedBy": "Remove this file manually or set AGENT_CLI_OVERRIDE_QUARANTINE=1"
    });

    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker_val).unwrap(),
    )
    .unwrap();

    let mut run_summary = None;
    if let Some(ref rid) = args.run_id {
        if let Some(mut run) = store.get(rid) {
            let ended_at = Utc::now().to_rfc3339();
            store
                .update(
                    rid,
                    AgentRunPatch {
                        status: Some("quarantined".to_string()),
                        ended_at: Some(ended_at.clone()),
                        ..Default::default()
                    },
                )
                .unwrap();
            run.status = "quarantined".to_string();
            run.ended_at = Some(ended_at);
            run_summary = Some(run);
        }
    }

    let report_md = format!(
        "# QUARANTINE REPORT\n\n\
         **Status:** QUARANTINED\n\
         **Directory:** {}\n\
         **Timestamp:** {}\n\n\
         ## Reason\n\n\
         {}\n\n\
         ## Affected Run\n\n\
         {}\n\n\
         ## What This Means\n\n\
         This directory has been quarantined. The `agent-cli-mcp` server will refuse \
         to dispatch further executor runs in this directory until the quarantine is cleared.\n\n\
         ## How to Clear\n\n\
         1. **Investigate** the issue described above.\n\
         2. **Review** any git diff for unexpected changes.\n\
         3. **Remove** the `.agent-cli-quarantine` file once satisfied:\n\
            ```bash\n\
            rm {}/.agent-cli-quarantine\n\
            ```\n\
         4. Alternatively, set `AGENT_CLI_OVERRIDE_QUARANTINE=1` for a one-time override (not recommended).\n\n\
         ## External Action Required\n\n\
         If secrets were exposed, credential rotation may be needed externally. \
         Do not attempt credential rotation through this MCP server.\n",
        resolved_cwd,
        Utc::now().to_rfc3339(),
        args.reason,
        match run_summary {
            Some(ref r) => format!(
                "- **Run ID:** {}\n- **Service:** {}\n- **Mode:** {}\n- **Command:** {}\n- **Started:** {}",
                r.id, r.service, r.mode, r.sanitized_command_summary, r.started_at
            ),
            None => "_No specific run ID provided._".to_string(),
        },
        resolved_cwd
    );

    fs::write(&report_path, report_md).unwrap();

    Ok(json!({
        "ok": true,
        "markerPath": marker_path.to_string_lossy(),
        "reportPath": report_path.to_string_lossy(),
        "message": "Quarantine applied. Remove .agent-cli-quarantine to clear."
    }))
}

// ── Artifacts Collection ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectArtifactsInput {
    pub cwd: String,
    pub run_id: Option<String>,
    pub include_diff: Option<bool>,
    pub include_result_files: Option<bool>,
    pub include_pr_info: Option<bool>,
}

pub async fn collect_artifacts(
    args: CollectArtifactsInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let resolved_cwd = validate_cwd(&args.cwd, config)?;

    // Changed files list
    let mut changed_files = Vec::new();
    if let Ok((true, out)) = spawn_for_capability(
        "git",
        &[
            String::from("-C"),
            resolved_cwd.clone(),
            String::from("diff"),
            String::from("HEAD"),
            String::from("--name-only"),
        ],
        10000,
    )
    .await
    {
        for l in out.lines() {
            if !l.trim().is_empty() {
                changed_files.push(l.trim().to_string());
            }
        }
    }

    if let Ok((true, out)) = spawn_for_capability(
        "git",
        &[
            String::from("-C"),
            resolved_cwd.clone(),
            String::from("ls-files"),
            String::from("--others"),
            String::from("--exclude-standard"),
        ],
        10000,
    )
    .await
    {
        for l in out.lines() {
            if !l.trim().is_empty() {
                changed_files.push(l.trim().to_string());
            }
        }
    }

    let mut diff_summary = None;
    if args.include_diff.unwrap_or(true) {
        if let Ok((true, out)) = spawn_for_capability(
            "git",
            &[
                String::from("-C"),
                resolved_cwd.clone(),
                String::from("diff"),
                String::from("HEAD"),
                String::from("--stat"),
            ],
            10000,
        )
        .await
        {
            let slice_end = std::cmp::min(out.len(), 5000);
            diff_summary = Some(redact_strict(&out[..slice_end]));
        }
    }

    let mut result_files = Vec::new();
    if args.include_result_files.unwrap_or(true) {
        if let Ok(entries) = fs::read_dir(&resolved_cwd) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if (name.ends_with(".md") || name.ends_with(".txt") || name.ends_with(".json"))
                    && !name.starts_with("README")
                    && !name.starts_with("CLAUDE")
                    && !name.starts_with("QUARANTINE")
                {
                    result_files.push(name);
                }
            }
        }
    }

    let mut pr_urls = Vec::new();
    if args.include_pr_info.unwrap_or(false) {
        if let Ok((true, out)) = spawn_for_capability(
            &config.gh_bin,
            &[
                String::from("pr"),
                String::from("list"),
                String::from("--json"),
                String::from("url"),
                String::from("--state"),
                String::from("open"),
                String::from("--limit"),
                String::from("3"),
            ],
            15000,
        )
        .await
        {
            if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&out) {
                for pr in parsed {
                    if let Some(url) = pr.get("url").and_then(|u| u.as_str()) {
                        pr_urls.push(url.to_string());
                    }
                }
            }
        }
    }

    let run_status = match args.run_id {
        Some(ref rid) => match store.get(rid) {
            Some(run) => format!(
                "Run {}: status={}, exitCode={:?}",
                run.id, run.status, run.exit_code
            ),
            None => "Run not found".to_string(),
        },
        None => "No run ID provided".to_string(),
    };

    Ok(json!({
        "ok": true,
        "changedFiles": changed_files,
        "diffSummary": diff_summary,
        "resultFiles": result_files,
        "prUrls": pr_urls,
        "verificationStatus": run_status
    }))
}

// ── Jules Remote Control Plane API integrations ───────────────────────────────

pub async fn list_jules_sources(config: &Config) -> Result<serde_json::Value, AgentCliError> {
    let client = JulesClient::new(config)?;
    let sources = client.list_sources(50, None).await?;
    Ok(json!({ "sources": sources }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesCreateSessionInput {
    pub cwd: String,
    pub prompt: String,
    pub source: String,
    pub title: Option<String>,
    pub starting_branch: Option<String>,
    pub require_plan_approval: Option<bool>,
    pub automation_mode: Option<String>,
}

pub async fn create_jules_session(
    args: JulesCreateSessionInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let validated_cwd = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&validated_cwd)?;

    let client = JulesClient::new(config)?;
    let req = CreateJulesSessionRequest {
        prompt: args.prompt.clone(),
        source_context: SourceContext {
            source: Some(args.source.clone()),
            github_repo_context: Some(GithubRepoContext {
                starting_branch: args.starting_branch.clone(),
            }),
            extra: serde_json::Value::Object(serde_json::Map::new()),
        },
        title: args.title.clone(),
        require_plan_approval: args.require_plan_approval,
        automation_mode: args.automation_mode.clone(),
    };

    let session = client.create_session(req).await?;
    let session_id = session.id.clone().unwrap_or_default();

    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).unwrap();

    let run = AgentRun {
        id: run_id.clone(),
        service: "jules".to_string(),
        mode: "api".to_string(),
        model: None,
        agent: None,
        cwd: validated_cwd.clone(),
        argv: vec![],
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: Some(session_id.clone()),
        remote_source: Some(args.source.clone()),
        remote_url: session.url.clone(),
        remote_state: session.state.clone(),
        normalized_status: Some(
            normalize_jules_session_state(session.state.as_deref())
                .as_str()
                .to_string(),
        ),
        last_activity_time: Some(Utc::now().to_rfc3339()),
        last_reconciled_at: Some(Utc::now().to_rfc3339()),
        stale_reason: None,
        sanitized_command_summary: format!("jules remote new (session={})", session_id),
    };

    store.save(&run).unwrap();

    let summary = build_jules_status(JulesStatusInput {
        session: Some(session),
        activities: vec![],
        run_id: Some(run_id),
        local_normalized_status: Some("running".to_string()),
        local_stale_reason: None,
        local_last_activity_time: Some(Utc::now().to_rfc3339()),
    });

    Ok(serde_json::to_value(&summary).unwrap())
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsArgs {
    pub include_activities: Option<bool>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetJulesStatusArgs {
    pub session_id: String,
    pub include_activities: Option<bool>,
    pub include_plan: Option<bool>,
    pub include_outputs: Option<bool>,
}

pub async fn list_jules_sessions(
    args: ListSessionsArgs,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let client = JulesClient::new(config)?;
    let sessions = client
        .list_sessions(args.page_size.unwrap_or(50), None)
        .await?;

    let mut summaries = Vec::new();
    let runs = store.list("jules", "all", Some(100));

    for s in sessions {
        let sid = s.id.clone().unwrap_or_default();
        let matching_run = runs
            .iter()
            .find(|r| r.remote_session_id.as_deref() == Some(&sid));

        let activities = if args.include_activities.unwrap_or(false) {
            client
                .list_activities(&sid, 10, None)
                .await
                .unwrap_or_default()
        } else {
            vec![]
        };

        let summary = build_jules_status(JulesStatusInput {
            session: Some(s),
            activities,
            run_id: matching_run.map(|r| r.id.clone()),
            local_normalized_status: matching_run.and_then(|r| r.normalized_status.clone()),
            local_stale_reason: matching_run.and_then(|r| r.stale_reason.clone()),
            local_last_activity_time: matching_run.and_then(|r| r.last_activity_time.clone()),
        });
        summaries.push(summary);
    }

    Ok(json!({ "sessions": summaries }))
}

pub async fn get_jules_status(
    args: GetJulesStatusArgs,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let client = JulesClient::new(config)?;
    let session = client.get_session(&args.session_id).await?;

    let activities = if args.include_activities.unwrap_or(true) {
        client
            .list_activities(&args.session_id, 100, None)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };

    let runs = store.list("jules", "all", Some(50));
    let matching_run = runs
        .iter()
        .find(|r| r.remote_session_id.as_deref() == Some(&args.session_id));

    let summary = build_jules_status(JulesStatusInput {
        session: Some(session),
        activities,
        run_id: matching_run.map(|r| r.id.clone()),
        local_normalized_status: matching_run.and_then(|r| r.normalized_status.clone()),
        local_stale_reason: matching_run.and_then(|r| r.stale_reason.clone()),
        local_last_activity_time: matching_run.and_then(|r| r.last_activity_time.clone()),
    });

    if let Some(run) = matching_run {
        let ns_str = summary.normalized_status.as_str().to_string();
        store
            .update(
                &run.id,
                AgentRunPatch {
                    normalized_status: Some(ns_str),
                    last_activity_time: summary.latest_activity_time.clone(),
                    last_reconciled_at: Some(Utc::now().to_rfc3339()),
                    remote_state: summary.state.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    Ok(serde_json::to_value(&summary).unwrap())
}

pub async fn approve_jules_plan(
    session_id: &str,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let client = JulesClient::new(config)?;
    client.approve_plan(session_id).await?;

    let runs = store.list("jules", "all", Some(50));
    if let Some(r) = runs
        .into_iter()
        .find(|run| run.remote_session_id.as_deref() == Some(session_id))
    {
        store
            .update(
                &r.id,
                AgentRunPatch {
                    normalized_status: Some("in_progress".to_string()),
                    last_activity_time: Some(Utc::now().to_rfc3339()),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    Ok(json!({ "ok": true, "message": format!("Approved plan for Jules session {}", session_id) }))
}

pub async fn send_jules_message(
    session_id: &str,
    prompt: &str,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let client = JulesClient::new(config)?;
    client.send_message(session_id, prompt).await?;

    let runs = store.list("jules", "all", Some(50));
    if let Some(r) = runs
        .into_iter()
        .find(|run| run.remote_session_id.as_deref() == Some(session_id))
    {
        store
            .update(
                &r.id,
                AgentRunPatch {
                    last_activity_time: Some(Utc::now().to_rfc3339()),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    Ok(
        json!({ "ok": true, "message": format!("Sent feedback message to Jules session {}", session_id) }),
    )
}

pub async fn list_jules_pending_actions(
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let client = JulesClient::new(config)?;
    let sessions = client.list_sessions(50, None).await?;

    let mut pending = Vec::new();
    let runs = store.list("jules", "all", Some(100));

    for s in sessions {
        let sid = s.id.clone().unwrap_or_default();
        let ns = normalize_jules_session_state(s.state.as_deref());

        if ns == NormalizedJulesStatus::AwaitingPlanApproval
            || ns == NormalizedJulesStatus::AwaitingUserFeedback
            || ns == NormalizedJulesStatus::Paused
        {
            let matching_run = runs
                .iter()
                .find(|r| r.remote_session_id.as_deref() == Some(&sid));
            let summary = build_jules_status(JulesStatusInput {
                session: Some(s),
                activities: vec![],
                run_id: matching_run.map(|r| r.id.clone()),
                local_normalized_status: matching_run.and_then(|r| r.normalized_status.clone()),
                local_stale_reason: matching_run.and_then(|r| r.stale_reason.clone()),
                local_last_activity_time: matching_run.and_then(|r| r.last_activity_time.clone()),
            });
            pending.push(summary);
        }
    }

    Ok(json!({ "pendingActions": pending }))
}

pub async fn reconcile_local_jules_runs(
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let client = match JulesClient::new(config) {
        Ok(c) => c,
        Err(_) => {
            return Ok(json!({ "staleRuns": [] }));
        }
    };

    let runs = store.all();
    let jules_runs: Vec<AgentRun> = runs
        .into_iter()
        .filter(|r| r.service == "jules" && r.status == "running")
        .collect();

    let mut stale_runs = Vec::new();
    for r in jules_runs {
        let is_stale = match r.remote_session_id {
            Some(ref sid) => match client.get_session(sid).await {
                Ok(s) => {
                    let ns = normalize_jules_session_state(s.state.as_deref());
                    !is_active_normalized_status(&ns)
                }
                Err(_) => true,
            },
            None => true,
        };

        if is_stale {
            store
                .update(
                    &r.id,
                    AgentRunPatch {
                        status: Some("failed".to_string()),
                        ended_at: Some(Utc::now().to_rfc3339()),
                        normalized_status: Some("stale_local_run".to_string()),
                        stale_reason: Some(
                            "Remote session does not exist or has already completed/failed"
                                .to_string(),
                        ),
                        ..Default::default()
                    },
                )
                .unwrap();
            stale_runs.push(r.id);
        }
    }

    Ok(json!({ "staleRuns": stale_runs }))
}

// ── Generic PR Listing via gh CLI ────────────────────────────────────────────

pub async fn gh_pr_list(cwd: &str, config: &Config) -> Result<serde_json::Value, AgentCliError> {
    let resolved = validate_cwd(cwd, config)?;

    let output = Command::new(&config.gh_bin)
        .args(["pr", "list", "--state", "open", "--limit", "10"])
        .current_dir(resolved)
        .output()
        .map_err(|e| {
            AgentCliError::new(
                ErrorCode::BackendFailed,
                &format!("Failed to spawn gh CLI: {}", e),
            )
        })?;

    let prs = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(json!({ "ok": true, "prs": redact_strict(&prs) }))
}

// ── Scoped Direct Binary Calling (L3 escape hatch) ───────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBinaryScopedInput {
    pub service: String, // "copilot" | "jules" | "gh"
    pub argv: Vec<String>,
    pub cwd: String,
    pub sandbox_level: String, // "target-repo" | "isolated-worktree" | "read-only"
    pub timeout_ms: Option<u64>,
}

pub async fn run_binary_scoped(
    args: RunBinaryScopedInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let allowed_scoped = [
        "copilot", "jules", "gh", "gemini", "codex", "opencode", "claude",
    ];
    if !allowed_scoped.contains(&args.service.as_str()) {
        return Err(AgentCliError::new(
            ErrorCode::InvalidService,
            &format!(
                "Invalid service for run_binary_scoped. Allowed: {}",
                allowed_scoped.join(", ")
            ),
        ));
    }

    let resolved_cwd = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&resolved_cwd)?;

    if args.sandbox_level == "isolated-worktree" {
        assert_is_worktree(&resolved_cwd)?;
    }

    let mut binary_args = args.argv.clone();
    validate_argv(&binary_args)?;

    if args.sandbox_level == "read-only" {
        binary_args.insert(0, "--dry-run".to_string());
    }

    let bin = resolve_binary(&args.service, config);

    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).unwrap();

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let run = AgentRun {
        id: run_id.clone(),
        service: args.service.clone(),
        mode: "custom".to_string(),
        model: None,
        agent: None,
        cwd: resolved_cwd.clone(),
        argv: binary_args.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: None,
        remote_source: None,
        remote_url: None,
        remote_state: None,
        normalized_status: None,
        last_activity_time: None,
        last_reconciled_at: None,
        stale_reason: None,
        sanitized_command_summary: format!("{} {}", bin, binary_args.join(" ")),
    };
    store.save(&run).unwrap();

    let spawn_opts = SpawnOptions {
        cwd: resolved_cwd,
        env,
        timeout_ms: Some(args.timeout_ms.unwrap_or(config.default_timeout_ms)),
        stdout_log_path: run.stdout_log.clone(),
        stderr_log_path: run.stderr_log.clone(),
        stdin_data: None,
    };

    let result = spawn_process(bin, &binary_args, spawn_opts).await?;

    let status_str = if result.exit_code == Some(0) {
        "complete"
    } else {
        "failed"
    };
    store
        .update(
            &run_id,
            AgentRunPatch {
                status: Some(status_str.to_string()),
                ended_at: Some(Utc::now().to_rfc3339()),
                exit_code: Some(result.exit_code),
                ..Default::default()
            },
        )
        .unwrap();

    Ok(json!({
        "ok": result.exit_code == Some(0),
        "runId": run_id,
        "exitCode": result.exit_code,
        "stdout": redact_strict(&result.stdout),
        "stderr": redact_strict(&result.stderr),
    }))
}

// ── Deprecated/Compatibility Jules remote shims ──────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesRemoteNewInput {
    pub cwd: String,
    pub prompt: String,
    pub starting_branch: Option<String>,
    pub argv: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
}

pub async fn jules_remote_new(
    args: JulesRemoteNewInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let resolved = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&resolved)?;

    let mut binary_args = vec!["remote".to_string(), "new".to_string(), args.prompt.clone()];
    if let Some(ref b) = args.starting_branch {
        binary_args.push("--branch".to_string());
        binary_args.push(b.clone());
    }

    if let Some(ref extra) = args.argv {
        validate_argv(extra)?;
        for a in extra {
            binary_args.push(a.clone());
        }
    }

    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).unwrap();

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let run = AgentRun {
        id: run_id.clone(),
        service: "jules".to_string(),
        mode: "remote".to_string(),
        model: None,
        agent: None,
        cwd: resolved.clone(),
        argv: binary_args.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: None,
        remote_source: None,
        remote_url: None,
        remote_state: None,
        normalized_status: None,
        last_activity_time: None,
        last_reconciled_at: None,
        stale_reason: None,
        sanitized_command_summary: format!("jules remote new prompt=\"{}\"", args.prompt),
    };
    store.save(&run).unwrap();

    let spawn_opts = SpawnOptions {
        cwd: resolved,
        env,
        timeout_ms: Some(args.timeout_ms.unwrap_or(config.default_timeout_ms)),
        stdout_log_path: run.stdout_log.clone(),
        stderr_log_path: run.stderr_log.clone(),
        stdin_data: None,
    };

    let result = spawn_process(&config.jules_bin, &binary_args, spawn_opts).await?;

    let status_str = if result.exit_code == Some(0) {
        "complete"
    } else {
        "failed"
    };
    store
        .update(
            &run_id,
            AgentRunPatch {
                status: Some(status_str.to_string()),
                ended_at: Some(Utc::now().to_rfc3339()),
                exit_code: Some(result.exit_code),
                ..Default::default()
            },
        )
        .unwrap();

    Ok(json!({
        "ok": result.exit_code == Some(0),
        "runId": run_id,
        "exitCode": result.exit_code,
        "stdout": redact_strict(&result.stdout),
        "stderr": redact_strict(&result.stderr),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesRemoteListInput {
    pub cwd: String,
}

pub async fn jules_remote_list(
    args: JulesRemoteListInput,
    config: &Config,
    _store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let resolved = validate_cwd(&args.cwd, config)?;

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let output = Command::new(&config.jules_bin)
        .args(["remote", "list"])
        .current_dir(resolved)
        .output()
        .map_err(|e| {
            AgentCliError::new(
                ErrorCode::BackendFailed,
                &format!("Failed to spawn jules remote list: {}", e),
            )
        })?;

    let list_out = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(json!({ "ok": output.status.success(), "sessions": redact_strict(&list_out) }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesRemoteApplyInput {
    pub cwd: String,
    pub session_id: String,
    pub timeout_ms: Option<u64>,
}

pub async fn jules_remote_apply(
    args: JulesRemoteApplyInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let resolved = validate_cwd(&args.cwd, config)?;
    assert_not_quarantined(&resolved)?;

    let binary_args = vec![
        "remote".to_string(),
        "pull".to_string(),
        "--session".to_string(),
        args.session_id.clone(),
        "--apply".to_string(),
    ];

    let run_id = generate_run_id();
    let (stdout_log, stderr_log) = store.allocate_log_paths(&run_id).unwrap();

    let mut env = HashMap::new();
    if let Ok(path_val) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path_val);
    }

    let run = AgentRun {
        id: run_id.clone(),
        service: "jules".to_string(),
        mode: "apply".to_string(),
        model: None,
        agent: None,
        cwd: resolved.clone(),
        argv: binary_args.clone(),
        status: "running".to_string(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        exit_code: None,
        stdout_log,
        stderr_log,
        prompt_file: None,
        worktree: None,
        remote_session_id: Some(args.session_id.clone()),
        remote_source: None,
        remote_url: None,
        remote_state: None,
        normalized_status: None,
        last_activity_time: None,
        last_reconciled_at: None,
        stale_reason: None,
        sanitized_command_summary: format!(
            "jules remote pull --session {} --apply",
            args.session_id
        ),
    };
    store.save(&run).unwrap();

    let spawn_opts = SpawnOptions {
        cwd: resolved,
        env,
        timeout_ms: Some(args.timeout_ms.unwrap_or(config.default_timeout_ms)),
        stdout_log_path: run.stdout_log.clone(),
        stderr_log_path: run.stderr_log.clone(),
        stdin_data: None,
    };

    let result = spawn_process(&config.jules_bin, &binary_args, spawn_opts).await?;

    let status_str = if result.exit_code == Some(0) {
        "complete"
    } else {
        "failed"
    };
    store
        .update(
            &run_id,
            AgentRunPatch {
                status: Some(status_str.to_string()),
                ended_at: Some(Utc::now().to_rfc3339()),
                exit_code: Some(result.exit_code),
                ..Default::default()
            },
        )
        .unwrap();

    Ok(json!({
        "ok": result.exit_code == Some(0),
        "runId": run_id,
        "exitCode": result.exit_code,
        "stdout": redact_strict(&result.stdout),
        "stderr": redact_strict(&result.stderr),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendInputArgs {
    pub session_id: String,
    pub input: String,
}

pub async fn send_input(
    args: SendInputArgs,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    session_manager
        .send_input(&args.session_id, &args.input)
        .await?;
    Ok(json!({ "ok": true, "bytesWritten": args.input.len() }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOutputArgs {
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub stream: Option<String>,
    pub mode: Option<String>,
    pub max_chars: Option<usize>,
}

pub fn read_output(
    args: ReadOutputArgs,
    config: &Config,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    let (stdout_log, stderr_log, status) = if let Some(ref sid) = args.session_id {
        if let Some(session) = session_manager.get_session_info(sid) {
            (
                Some(session.stdout_log_path.clone()),
                Some(session.stderr_log_path.clone()),
                Some(session.status.clone()),
            )
        } else {
            return Err(AgentCliError::new(
                ErrorCode::SessionNotFound,
                &format!("Session not found: {}", sid),
            ));
        }
    } else if let Some(ref rid) = args.run_id {
        if let Some(run) = store.get(rid) {
            (
                Some(run.stdout_log.clone()),
                Some(run.stderr_log.clone()),
                Some(run.status.clone()),
            )
        } else {
            return Err(AgentCliError::new(
                ErrorCode::SessionNotFound,
                &format!("Run not found: {}", rid),
            ));
        }
    } else {
        return Err(AgentCliError::new(
            ErrorCode::SessionNotFound,
            "Provide either runId or sessionId",
        ));
    };
    let stream = args.stream.as_deref().unwrap_or("both");
    let max_chars = args.max_chars.unwrap_or(config.max_output_chars);

    let mut raw = String::new();
    if stream == "stdout" || stream == "both" {
        if let Some(ref path) = stdout_log {
            if Path::new(path).exists() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    raw.push_str(&content);
                }
            }
        }
    }
    if stream == "stderr" || stream == "both" {
        if let Some(ref path) = stderr_log {
            if Path::new(path).exists() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if !raw.is_empty() {
                        raw.push_str("\n[stderr]\n");
                    }
                    raw.push_str(&content);
                }
            }
        }
    }

    let redacted = redact_strict(&raw);
    let mode = args.mode.as_deref().unwrap_or("tail");
    let output = if mode == "tail" {
        if redacted.len() > max_chars {
            redacted[redacted.len() - max_chars..].to_string()
        } else {
            redacted
        }
    } else {
        if redacted.len() > max_chars {
            redacted[..max_chars].to_string()
        } else {
            redacted
        }
    };

    Ok(json!({
        "ok": true,
        "output": output,
        "status": status,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsInput {
    pub service: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
}

pub fn list_sessions(
    args: ListSessionsInput,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    let service_filter = match args.service.as_deref() {
        Some("all") | None => None,
        Some(s) => Some(s),
    };
    let status_filter = match args.status.as_deref() {
        Some("all") | None => None,
        Some(s) => Some(s),
    };
    let limit = args.limit.unwrap_or(20);

    let runs = store.list(
        service_filter.unwrap_or("all"),
        status_filter.unwrap_or("all"),
        Some(limit),
    );

    let live_sessions = session_manager.list();
    let mut live_by_run_id = HashMap::new();
    for s in &live_sessions {
        if let Some(ref rid) = s.run_id {
            live_by_run_id.insert(rid.clone(), s.clone());
        }
    }

    let result_list: Vec<serde_json::Value> = runs
        .into_iter()
        .map(|r| {
            let live = live_by_run_id.get(&r.id);
            json!({
                "id": r.id,
                "sessionId": live.map(|l| l.id.clone()),
                "remoteSessionId": r.remote_session_id,
                "service": r.service,
                "mode": r.mode,
                "status": live.map(|l| l.status.clone()).unwrap_or(r.status),
                "remoteState": r.remote_state,
                "normalizedStatus": r.normalized_status,
                "startedAt": r.started_at,
                "endedAt": r.ended_at,
                "exitCode": r.exit_code,
                "cwd": r.cwd,
                "remoteUrl": r.remote_url,
                "lastActivityTime": r.last_activity_time,
                "staleReason": r.stale_reason,
                "sanitizedCommandSummary": r.sanitized_command_summary,
            })
        })
        .collect();

    Ok(serde_json::Value::Array(result_list))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KillSessionArgs {
    pub session_id: String,
    pub reason: Option<String>,
}

pub async fn kill_session(
    args: KillSessionArgs,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    if let Some(live) = session_manager.get_session_info(&args.session_id) {
        session_manager
            .kill(&args.session_id, args.reason.as_deref())
            .await?;
        if let Some(ref rid) = live.run_id {
            store
                .update(
                    rid,
                    AgentRunPatch {
                        status: Some("killed".to_string()),
                        ended_at: Some(Utc::now().to_rfc3339()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        return Ok(json!({ "ok": true, "message": format!("Session {} killed", args.session_id) }));
    }

    if let Some(_run) = store.get(&args.session_id) {
        let live_match = session_manager
            .list()
            .into_iter()
            .find(|s| s.run_id.as_deref() == Some(&args.session_id));
        if let Some(live) = live_match {
            session_manager
                .kill(&live.id, args.reason.as_deref())
                .await?;
        }
        store
            .update(
                &args.session_id,
                AgentRunPatch {
                    status: Some("killed".to_string()),
                    ended_at: Some(Utc::now().to_rfc3339()),
                    ..Default::default()
                },
            )
            .unwrap();
        return Ok(json!({ "ok": true, "message": format!("Run {} killed", args.session_id) }));
    }

    Err(AgentCliError::new(
        ErrorCode::SessionNotFound,
        &format!("No session or run found with ID: {}", args.session_id),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotRunInput {
    pub cwd: String,
    pub mode: Option<String>,
    pub prompt: Option<String>,
    pub argv: Option<Vec<String>>,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    pub dry_run: Option<bool>,
}

pub async fn copilot_run(
    input: CopilotRunInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    execute_run(
        RunInput {
            service: "copilot".to_string(),
            mode: input.mode.unwrap_or_else(|| "prompt".to_string()),
            cwd: input.cwd,
            prompt: input.prompt,
            argv: input.argv,
            timeout_ms: input.timeout_ms,
            allow_tools: input.allow_tools,
            deny_tools: input.deny_tools,
            allow_env_writes: None,
            dry_run: input.dry_run,
        },
        config,
        store,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotFleetInput {
    pub cwd: String,
    pub prompt: String,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
}

pub async fn copilot_fleet(
    input: CopilotFleetInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    execute_run(
        RunInput {
            service: "copilot".to_string(),
            mode: "fleet".to_string(),
            cwd: input.cwd,
            prompt: Some(input.prompt),
            argv: None,
            timeout_ms: input.timeout_ms,
            allow_tools: input.allow_tools,
            deny_tools: input.deny_tools,
            allow_env_writes: None,
            dry_run: None,
        },
        config,
        store,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotDelegateInput {
    pub cwd: String,
    pub prompt: String,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
}

pub async fn copilot_delegate(
    input: CopilotDelegateInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    execute_run(
        RunInput {
            service: "copilot".to_string(),
            mode: "prompt".to_string(),
            cwd: input.cwd,
            prompt: Some(format!("/delegate {}", input.prompt)),
            argv: None,
            timeout_ms: input.timeout_ms,
            allow_tools: input.allow_tools,
            deny_tools: input.deny_tools,
            allow_env_writes: None,
            dry_run: None,
        },
        config,
        store,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotAutopilotInput {
    pub cwd: String,
    pub prompt: String,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    pub background: Option<bool>,
}

pub async fn copilot_autopilot(
    input: CopilotAutopilotInput,
    config: &Config,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    let run_in = RunInput {
        service: "copilot".to_string(),
        mode: "autopilot".to_string(),
        cwd: input.cwd,
        prompt: Some(input.prompt),
        argv: None,
        timeout_ms: input.timeout_ms,
        allow_tools: input.allow_tools,
        deny_tools: input.deny_tools,
        allow_env_writes: None,
        dry_run: None,
    };
    if input.background.unwrap_or(false) {
        start_run(run_in, config, store, session_manager).await
    } else {
        execute_run(run_in, config, store).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotKeepAliveInput {
    pub cwd: String,
    pub prompt: String,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
}

pub async fn copilot_keep_alive(
    input: CopilotKeepAliveInput,
    config: &Config,
    store: &Store,
    session_manager: &SessionManager,
) -> Result<serde_json::Value, AgentCliError> {
    start_run(
        RunInput {
            service: "copilot".to_string(),
            mode: "prompt".to_string(),
            cwd: input.cwd,
            prompt: Some(format!("/keep-alive {}", input.prompt)),
            argv: None,
            timeout_ms: input.timeout_ms,
            allow_tools: input.allow_tools,
            deny_tools: input.deny_tools,
            allow_env_writes: None,
            dry_run: None,
        },
        config,
        store,
        session_manager,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopilotReviewInput {
    pub cwd: String,
    pub prompt: String,
    pub allow_tools: Option<Vec<String>>,
    pub deny_tools: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
}

pub async fn copilot_review(
    input: CopilotReviewInput,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    execute_run(
        RunInput {
            service: "copilot".to_string(),
            mode: "prompt".to_string(),
            cwd: input.cwd,
            prompt: Some(format!("/review {}", input.prompt)),
            argv: None,
            timeout_ms: input.timeout_ms,
            allow_tools: Some(
                input
                    .allow_tools
                    .unwrap_or_else(|| vec!["read".to_string()]),
            ),
            deny_tools: input.deny_tools,
            allow_env_writes: None,
            dry_run: None,
        },
        config,
        store,
    )
    .await
}

pub async fn request_jules_verification(
    session_id: &str,
    test_command: &str,
    test_cwd: Option<String>,
    _timeout_ms: Option<u64>,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let message = format!(
        "Please verify the current result before requesting review.\n\nRequired evidence:\n- Run verification command: `{}` in directory `{:?}`\n",
        test_command,
        test_cwd.as_deref().unwrap_or("repo root")
    );

    let client = JulesClient::new(config)?;
    client.send_message(session_id, &message).await?;
    get_jules_status(
        GetJulesStatusArgs {
            session_id: session_id.to_string(),
            include_activities: Some(true),
            include_plan: Some(true),
            include_outputs: Some(true),
        },
        config,
        store,
    )
    .await
}

pub async fn collect_jules_outputs(
    session_id: &str,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let status = get_jules_status(
        GetJulesStatusArgs {
            session_id: session_id.to_string(),
            include_activities: Some(true),
            include_plan: Some(true),
            include_outputs: Some(true),
        },
        config,
        store,
    )
    .await?;

    let client = JulesClient::new(config)?;
    let activities = client
        .list_activities(session_id, 100, None)
        .await
        .unwrap_or_default();

    Ok(json!({
        "ok": true,
        "sessionId": session_id,
        "outputs": status.get("outputs").cloned().unwrap_or_else(|| json!([])),
        "pr": status.get("pr").cloned().unwrap_or(serde_json::Value::Null),
        "activities": activities,
    }))
}

pub async fn watch_jules_session(
    session_id: &str,
    until_states: &[String],
    timeout_ms: Option<u64>,
    config: &Config,
    store: &Store,
) -> Result<serde_json::Value, AgentCliError> {
    let until_set: std::collections::HashSet<&str> =
        until_states.iter().map(|s| s.as_str()).collect();
    let poll_interval = std::time::Duration::from_secs(5);
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000));
    let started_at = std::time::Instant::now();

    while started_at.elapsed() < timeout {
        let status_val = get_jules_status(
            GetJulesStatusArgs {
                session_id: session_id.to_string(),
                include_activities: Some(true),
                include_plan: Some(true),
                include_outputs: Some(true),
            },
            config,
            store,
        )
        .await?;

        if let Some(ns) = status_val.get("normalizedStatus").and_then(|v| v.as_str()) {
            if until_set.contains(ns) {
                return Ok(json!({ "ok": true, "status": status_val }));
            }
        }

        tokio::time::sleep(poll_interval).await;
    }

    Err(AgentCliError::new(
        ErrorCode::RunTimeout,
        &format!("Timed out while waiting for Jules session {}.", session_id),
    ))
}
