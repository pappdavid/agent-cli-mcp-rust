use crate::config::Config;
use crate::errors::AgentCliError;
use crate::policy::validate_cwd;
use crate::redaction::redact;
use crate::store::Store;
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct SanityFinding {
    #[serde(rename = "type")]
    pub finding_type: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanityResult {
    pub ok: bool,
    pub severity: String, // "ok" | "warning" | "quarantine"
    pub findings: Vec<SanityFinding>,
    #[serde(rename = "recommendedAction")]
    pub recommended_action: String,
}

pub async fn run_sanity_check(
    cwd: &str,
    run_id: Option<&str>,
    include_git_diff: Option<bool>,
    include_logs: Option<bool>,
    config: &Config,
    store: &Store,
) -> Result<SanityResult, AgentCliError> {
    let resolved_cwd = validate_cwd(cwd, config)?;
    let mut findings = Vec::new();

    let dangerous_command_re = Regex::new(
        r"(?i)vercel\s+(deploy\s+--prod|env\s+add|env\s+rm)|supabase\s+(db\s+reset|projects\s+(pause|delete|create)|secrets)|security\s+find-generic-password|git\s+push\s+--force|credential\s+rotat|rotate.*key|revoke.*token"
    ).unwrap();

    let context_loss_re = Regex::new(
        r"(?i)(?:i don't have context|i lost track|starting over|i cannot proceed|repeated fail|same error again|i give up)"
    ).unwrap();

    // ── Git diff analysis ────────────────────────────────────────────────────
    if include_git_diff.unwrap_or(true) {
        let diff_files_out = Command::new("git")
            .args(["-C", &resolved_cwd, "diff", "HEAD", "--name-only"])
            .output();

        if let Ok(out) = diff_files_out {
            if out.status.success() {
                let files_str = String::from_utf8_lossy(&out.stdout);
                let changed_files: Vec<&str> = files_str
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect();

                for file in changed_files {
                    let path = Path::new(file);
                    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    if basename.starts_with(".env") {
                        findings.push(SanityFinding {
                            finding_type: "ENV_FILE_MODIFIED".to_string(),
                            message: format!("Environment file modified: {}", file),
                            file: Some(file.to_string()),
                            evidence: Some(
                                "Executor wrote to a .env file. Verify no secrets were embedded."
                                    .to_string(),
                            ),
                        });
                    }

                    if basename.to_lowercase() == "claude.md" {
                        let file_diff_out = Command::new("git")
                            .args(["-C", &resolved_cwd, "diff", "HEAD", "--", file])
                            .output();

                        if let Ok(fd_out) = file_diff_out {
                            let diff_content =
                                String::from_utf8_lossy(&fd_out.stdout).to_lowercase();
                            if diff_content.contains("secret") || diff_content.contains("key") {
                                findings.push(SanityFinding {
                                    finding_type: "CLAUDE_MD_SUSPICIOUS_EDIT".to_string(),
                                    message: "CLAUDE.md was modified and may contain secrets or policy edits".to_string(),
                                    file: Some(file.to_string()),
                                    evidence: None,
                                });
                            }
                        }
                    }

                    if file.starts_with(".vercel/") {
                        findings.push(SanityFinding {
                            finding_type: "VERCEL_CONFIG_MODIFIED".to_string(),
                            message: format!("Vercel config modified: {}", file),
                            file: Some(file.to_string()),
                            evidence: None,
                        });
                    }

                    if file.starts_with("supabase/") {
                        findings.push(SanityFinding {
                            finding_type: "SUPABASE_CONFIG_MODIFIED".to_string(),
                            message: format!("Supabase config modified: {}", file),
                            file: Some(file.to_string()),
                            evidence: None,
                        });
                    }
                }

                // Full diff for secret scanning
                let full_diff_out = Command::new("git")
                    .args(["-C", &resolved_cwd, "diff", "HEAD"])
                    .output();

                if let Ok(fd_out) = full_diff_out {
                    let full_diff = String::from_utf8_lossy(&fd_out.stdout);
                    // Take first 100 KB
                    let slice_len = std::cmp::min(full_diff.len(), 100_000);
                    let diff_slice = &full_diff[..slice_len];

                    let redact_res = redact(diff_slice);
                    if redact_res.triggered {
                        findings.push(SanityFinding {
                            finding_type: "SECRET_IN_GIT_DIFF".to_string(),
                            message: "Secret-looking string detected in git diff (redacted in this report)".to_string(),
                            file: None,
                            evidence: Some("[REDACTED — check raw diff locally]".to_string()),
                        });
                    }

                    if dangerous_command_re.is_match(&full_diff) {
                        findings.push(SanityFinding {
                            finding_type: "DANGEROUS_COMMAND_IN_DIFF".to_string(),
                            message: "Dangerous provider command found in diff".to_string(),
                            file: None,
                            evidence: Some(extract_match(&full_diff, &dangerous_command_re)),
                        });
                    }
                }
            }
        }
    }

    // ── Log analysis ─────────────────────────────────────────────────────────
    if include_logs.unwrap_or(true) {
        if let Some(rid) = run_id {
            if let Some(run) = store.get(rid) {
                for log_path_str in &[&run.stdout_log, &run.stderr_log] {
                    let path = Path::new(log_path_str);
                    if !path.exists() {
                        continue;
                    }

                    if let Ok(raw) = fs::read_to_string(path) {
                        let slice_len = std::cmp::min(raw.len(), 200_000);
                        let raw_slice = &raw[..slice_len];

                        let redact_res = redact(raw_slice);
                        if redact_res.triggered {
                            findings.push(SanityFinding {
                                finding_type: "SECRET_IN_LOGS".to_string(),
                                message: format!(
                                    "Secret-looking string found in logs: {}",
                                    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
                                ),
                                file: Some((*log_path_str).clone()),
                                evidence: None,
                            });
                        }

                        if dangerous_command_re.is_match(raw_slice) {
                            findings.push(SanityFinding {
                                finding_type: "DANGEROUS_COMMAND_IN_LOGS".to_string(),
                                message: "Dangerous provider command detected in executor logs"
                                    .to_string(),
                                file: Some((*log_path_str).clone()),
                                evidence: Some(extract_match(raw_slice, &dangerous_command_re)),
                            });
                        }

                        if context_loss_re.is_match(raw_slice) {
                            findings.push(SanityFinding {
                                finding_type: "EXECUTOR_CONTEXT_LOSS".to_string(),
                                message: "Executor output suggests context loss or repeated failure loops".to_string(),
                                file: Some((*log_path_str).clone()),
                                evidence: Some(extract_match(raw_slice, &context_loss_re)),
                            });
                        }
                    }
                }
            }
        }
    }

    // ── Determine severity ───────────────────────────────────────────────────
    let mut severity = "ok";
    for f in &findings {
        if f.finding_type == "SECRET_IN_GIT_DIFF"
            || f.finding_type == "SECRET_IN_LOGS"
            || f.finding_type == "DANGEROUS_COMMAND_IN_DIFF"
            || f.finding_type == "DANGEROUS_COMMAND_IN_LOGS"
        {
            severity = "quarantine";
            break;
        } else if f.finding_type == "ENV_FILE_MODIFIED"
            || f.finding_type == "CLAUDE_MD_SUSPICIOUS_EDIT"
            || f.finding_type == "VERCEL_CONFIG_MODIFIED"
            || f.finding_type == "SUPABASE_CONFIG_MODIFIED"
            || f.finding_type == "EXECUTOR_CONTEXT_LOSS"
        {
            severity = "warning";
        }
    }

    let recommended_action = match severity {
        "quarantine" => "STOP. Call agent_cli.quarantine on this directory. Do not dispatch further executor runs until the issue is resolved and the quarantine is manually cleared.",
        "warning" => "Review the findings carefully before proceeding. Consider reverting suspicious changes.",
        _ => "No issues detected. Safe to proceed.",
    };

    Ok(SanityResult {
        ok: severity == "ok",
        severity: severity.to_string(),
        findings,
        recommended_action: recommended_action.to_string(),
    })
}

fn extract_match(text: &str, re: &Regex) -> String {
    if let Some(m) = re.find(text) {
        let matched = m.as_str();
        let end = std::cmp::min(matched.len(), 200);
        matched[..end].to_string()
    } else {
        String::new()
    }
}
