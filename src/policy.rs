use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::collections::HashSet;
use regex::Regex;
use crate::config::{Config, canonicalize_path};
use crate::errors::{AgentCliError, ErrorCode};

pub const VALID_SERVICES: &[&str] = &[
    "copilot",
    "jules",
    "gemini",
    "codex",
    "opencode",
    "claude",
];
pub const VALID_MODES: &[&str] = &[
    "prompt",
    "interactive",
    "autopilot",
    "fleet",
    "resume",
    "remote",
    "api",
    "status",
    "apply",
    "custom",
];

pub const MANDATORY_DENY_OVERLAY: &[&str] = &[
    "memory",
    "shell(vercel env:*)",
    "shell(vercel deploy --prod)",
    "shell(supabase projects:*)",
    "shell(supabase db reset)",
    "shell(supabase secrets:*)",
    "shell(security find-generic-password)",
    "shell(git push --force)",
];

pub const ORCHESTRATOR_CONTROLLED_DENY: &[&str] = &["write(.env*)"];

pub struct ToolPreset {
    pub allow_tools: Vec<String>,
    pub deny_tools: Vec<String>,
}

pub fn get_tool_preset(name: &str) -> Option<ToolPreset> {
    match name {
        "copilot-file-edit" => Some(ToolPreset {
            allow_tools: vec![
                "read".to_string(),
                "write(src/**)".to_string(),
                "write(test/**)".to_string(),
                "write(tests/**)".to_string(),
                "write(lib/**)".to_string(),
                "write(docs/**)".to_string(),
                "shell(git diff)".to_string(),
                "shell(git status)".to_string(),
                "shell(git log)".to_string(),
                "shell(git add)".to_string(),
                "shell(git commit)".to_string(),
            ],
            deny_tools: vec![
                "memory".to_string(),
                "write(.env*)".to_string(),
                "write(.vercel/**)".to_string(),
                "shell(vercel env:*)".to_string(),
                "shell(vercel deploy --prod)".to_string(),
                "shell(supabase projects:*)".to_string(),
                "shell(supabase db reset)".to_string(),
                "shell(supabase secrets:*)".to_string(),
                "shell(security find-generic-password)".to_string(),
                "shell(git push --force)".to_string(),
            ],
        }),
        "copilot-safe-dev" => Some(ToolPreset {
            allow_tools: vec![
                "read".to_string(),
                "write(src/**)".to_string(),
                "write(test/**)".to_string(),
                "write(tests/**)".to_string(),
                "shell(git:*)".to_string(),
                "shell(pnpm:*)".to_string(),
                "shell(npm:*)".to_string(),
                "shell(node:*)".to_string(),
            ],
            deny_tools: vec![
                "memory".to_string(),
                "write(.env*)".to_string(),
                "shell(git push --force)".to_string(),
                "shell(vercel:*)".to_string(),
                "shell(supabase:*)".to_string(),
                "shell(security find-generic-password)".to_string(),
            ],
        }),
        "copilot-expanded-worktree" => Some(ToolPreset {
            allow_tools: vec![
                "read".to_string(),
                "write(**)".to_string(),
                "shell(git:*)".to_string(),
                "shell(pnpm:*)".to_string(),
                "shell(npm:*)".to_string(),
                "shell(node:*)".to_string(),
            ],
            deny_tools: vec![
                "memory".to_string(),
                "write(.env*)".to_string(),
                "shell(vercel env:*)".to_string(),
                "shell(vercel deploy --prod)".to_string(),
                "shell(supabase projects:*)".to_string(),
                "shell(supabase db reset)".to_string(),
                "shell(security find-generic-password)".to_string(),
            ],
        }),
        "copilot-target-repo" => Some(ToolPreset {
            allow_tools: vec![
                "read".to_string(),
                "write(**)".to_string(),
                "shell(git:*)".to_string(),
                "shell(npm:*)".to_string(),
                "shell(pnpm:*)".to_string(),
                "shell(yarn:*)".to_string(),
                "shell(bun:*)".to_string(),
                "shell(node:*)".to_string(),
                "shell(npx:*)".to_string(),
                "shell(make:*)".to_string(),
                "shell(cargo:*)".to_string(),
                "shell(go:*)".to_string(),
                "shell(python:*)".to_string(),
                "shell(pytest:*)".to_string(),
                "shell(jest:*)".to_string(),
                "shell(vitest:*)".to_string(),
            ],
            deny_tools: vec![
                "memory".to_string(),
                "write(.env*)".to_string(),
                "shell(vercel env:*)".to_string(),
                "shell(vercel deploy --prod)".to_string(),
                "shell(supabase projects:*)".to_string(),
                "shell(supabase db reset)".to_string(),
                "shell(supabase secrets:*)".to_string(),
                "shell(security find-generic-password)".to_string(),
                "shell(git push --force)".to_string(),
            ],
        }),
        _ => None,
    }
}

pub fn validate_service(service: &str) -> Result<(), AgentCliError> {
    if !VALID_SERVICES.contains(&service) {
        return Err(AgentCliError::new(
            ErrorCode::InvalidService,
            &format!("Invalid service \"{}\". Allowed: {}", service, VALID_SERVICES.join(", ")),
        ));
    }
    Ok(())
}

pub fn validate_mode(mode: &str) -> Result<(), AgentCliError> {
    if !VALID_MODES.contains(&mode) {
        return Err(AgentCliError::new(
            ErrorCode::InvalidMode,
            &format!("Invalid mode \"{}\". Allowed: {}", mode, VALID_MODES.join(", ")),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct GitContext {
    pub repo_root: String,
    pub canonical_repo_root: String,
}

pub fn detect_git_context(cwd: &str) -> Option<GitContext> {
    let repo_root_out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !repo_root_out.status.success() {
        return None;
    }

    let repo_root = String::from_utf8_lossy(&repo_root_out.stdout).trim().to_string();
    let repo_root_canonical = canonicalize_path(&repo_root);

    let mut common_dir = String::new();
    if let Ok(common_dir_out) = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .output()
    {
        if common_dir_out.status.success() {
            common_dir = String::from_utf8_lossy(&common_dir_out.stdout).trim().to_string();
        }
    }

    if common_dir.is_empty() {
        if let Ok(common_dir_out) = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(cwd)
            .output()
        {
            if common_dir_out.status.success() {
                let parsed = String::from_utf8_lossy(&common_dir_out.stdout).trim().to_string();
                let path = Path::new(&parsed);
                if path.is_absolute() {
                    common_dir = parsed;
                } else {
                    common_dir = Path::new(&repo_root).join(path).to_string_lossy().to_string();
                }
            }
        }
    }

    let canonical_repo_root = if !common_dir.is_empty() {
        let path = Path::new(&common_dir);
        let parent = path.parent();
        if path.file_name().and_then(|n| n.to_str()) == Some(".git") && parent.is_some() {
            canonicalize_path(&parent.unwrap().to_string_lossy())
        } else {
            repo_root_canonical.clone()
        }
    } else {
        repo_root_canonical.clone()
    };

    Some(GitContext {
        repo_root: repo_root_canonical,
        canonical_repo_root,
    })
}

pub fn validate_cwd(cwd: &str, config: &Config) -> Result<String, AgentCliError> {
    let resolved = canonicalize_path(cwd);
    if !Path::new(&resolved).exists() {
        return Err(AgentCliError::new(
            ErrorCode::CwdNotAllowed,
            &format!("Directory does not exist: {}", resolved),
        ));
    }

    let git_context = detect_git_context(&resolved);
    let allowed = config.allowed_roots.iter().any(|root| {
        let resolved_root = canonicalize_path(root);
        is_within_root(&resolved, &resolved_root)
            || git_context.as_ref().map_or(false, |ctx| {
                is_within_root(&ctx.repo_root, &resolved_root)
                    || is_within_root(&ctx.canonical_repo_root, &resolved_root)
            })
    });

    if !allowed {
        return Err(AgentCliError::new(
            ErrorCode::CwdNotAllowed,
            &format!(
                "\"{}\" is not under any allowed root. Configure AGENT_CLI_ALLOWED_ROOTS.",
                resolved
            ),
        ));
    }

    Ok(resolved)
}

fn is_within_root(candidate: &str, root: &str) -> bool {
    let cand_path = Path::new(candidate);
    let root_path = Path::new(root);
    candidate == root || cand_path.starts_with(root_path)
}

pub fn validate_argv(argv: &[String]) -> Result<(), AgentCliError> {
    let control_char_re = Regex::new(r"[\x00\r\n]").unwrap();
    let shell_sequence_re = Regex::new(r"&&|\|\||`|\$\(|\|>|[;<>|]").unwrap();

    let shell_launchers: HashSet<&str> = ["bash", "sh", "zsh"].into_iter().collect();
    let shell_launcher_flags: HashSet<&str> = ["-c", "-lc", "-ic"].into_iter().collect();
    let structured_tool_flag_names: HashSet<&str> = [
        "--allow-tool",
        "--deny-tool",
        "--available-tools",
        "--excluded-tools",
    ]
    .into_iter()
    .collect();
    let free_text_flag_names: HashSet<&str> = ["-p", "--prompt", "-i", "--interactive"].into_iter().collect();

    let mut skip_next_free_text_value = false;

    for i in 0..argv.len() {
        let arg = &argv[i];

        if skip_next_free_text_value {
            skip_next_free_text_value = false;
            continue;
        }

        if free_text_flag_names.contains(arg.as_str()) {
            skip_next_free_text_value = true;
            continue;
        }

        if arg.starts_with("--prompt=") || arg.starts_with("--interactive=") {
            continue;
        }

        if control_char_re.is_match(arg) {
            return Err(AgentCliError::new(
                ErrorCode::CommandBlocked,
                &format!("Argument contains control characters and was rejected: {:?}", arg),
            ));
        }

        if shell_launchers.contains(arg.as_str()) {
            if let Some(next_arg) = argv.get(i + 1) {
                if shell_launcher_flags.contains(next_arg.as_str()) {
                    let seq = argv.iter().skip(i).take(3).cloned().collect::<Vec<_>>();
                    return Err(AgentCliError::new(
                        ErrorCode::CommandBlocked,
                        &format!("Shell launcher sequence is not allowed: {:?}", seq),
                    ));
                }
            }
        }

        if let Some((flag_name, value)) = parse_structured_tool_flag(arg) {
            validate_structured_arg_value(&value, &flag_name)?;
            continue;
        }

        if structured_tool_flag_names.contains(arg.as_str()) {
            if let Some(next_arg) = argv.get(i + 1) {
                validate_structured_arg_value(next_arg, arg)?;
            }
        }

        if shell_sequence_re.is_match(arg) {
            return Err(AgentCliError::new(
                ErrorCode::CommandBlocked,
                &format!("Argument contains shell execution syntax and was rejected: {:?}", arg),
            ));
        }
    }

    Ok(())
}

fn parse_structured_tool_flag(arg: &str) -> Option<(String, String)> {
    let re = Regex::new(r"^(--allow-tool|--deny-tool|--available-tools|--excluded-tools)=(.+)$").unwrap();
    let caps = re.captures(arg)?;
    Some((caps.get(1)?.as_str().to_string(), caps.get(2)?.as_str().to_string()))
}

fn validate_structured_arg_value(value: &str, flag_name: &str) -> Result<(), AgentCliError> {
    let control_char_re = Regex::new(r"[\x00\r\n]").unwrap();
    let shell_sequence_re = Regex::new(r"&&|\|\||`|\$\(|\|>|[;<>|]").unwrap();

    if control_char_re.is_match(value) || shell_sequence_re.is_match(value) {
        return Err(AgentCliError::new(
            ErrorCode::CommandBlocked,
            &format!(
                "Structured {} value contains forbidden shell syntax: {:?}",
                flag_name, value
            ),
        ));
    }

    if value.contains("shell(") {
        let shell_launcher_re = Regex::new(r"\b(?:bash|sh|zsh)\s+-l?c\b").unwrap();
        if shell_launcher_re.is_match(value) {
            return Err(AgentCliError::new(
                ErrorCode::CommandBlocked,
                &format!(
                    "Structured {} value contains a shell launcher: {:?}",
                    flag_name, value
                ),
            ));
        }
    }

    Ok(())
}

pub fn merge_tool_permissions(
    user_allow: &[String],
    user_deny: &[String],
    allow_env_writes: bool,
) -> (Vec<String>, Vec<String>) {
    let mut base_deny = MANDATORY_DENY_OVERLAY
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    if !allow_env_writes {
        for s in ORCHESTRATOR_CONTROLLED_DENY {
            base_deny.push(s.to_string());
        }
    }

    for d in user_deny {
        if !base_deny.contains(d) {
            base_deny.push(d.clone());
        }
    }

    (user_allow.to_vec(), base_deny)
}

pub fn assert_is_worktree(cwd: &str) -> Result<(), AgentCliError> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(cwd)
        .output();

    match out {
        Ok(output) if output.status.success() => {
            let git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if git_dir == ".git" || git_dir.ends_with("/.git") {
                return Err(AgentCliError::new(
                    ErrorCode::NotAWorktree,
                    &format!("{} is the main checkout, not an isolated worktree", cwd),
                ));
            }
            Ok(())
        }
        _ => Err(AgentCliError::new(
            ErrorCode::NotAWorktree,
            &format!("Cannot determine git context for {}", cwd),
        )),
    }
}

pub fn is_quarantined(cwd: &str) -> bool {
    Path::new(cwd).join(".agent-cli-quarantine").exists()
}

pub fn assert_not_quarantined(cwd: &str) -> Result<(), AgentCliError> {
    if is_quarantined(cwd) {
        return Err(AgentCliError::new(
            ErrorCode::Quarantined,
            &format!(
                "Directory is quarantined: {}. Remove .agent-cli-quarantine to clear.",
                cwd
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_service() {
        assert!(validate_service("copilot").is_ok());
        assert!(validate_service("jules").is_ok());
        assert!(validate_service("gemini").is_ok());
        assert!(validate_service("codex").is_ok());
        assert!(validate_service("opencode").is_ok());
        assert!(validate_service("claude").is_ok());
        assert!(validate_service("invalid_service").is_err());
    }

    #[test]
    fn test_validate_mode() {
        assert!(validate_mode("prompt").is_ok());
        assert!(validate_mode("autopilot").is_ok());
        assert!(validate_mode("invalid_mode").is_err());
    }

    #[test]
    fn test_validate_argv_safe() {
        let argv = vec![
            "copilot".to_string(),
            "run".to_string(),
            "-p".to_string(),
            "some query".to_string(),
        ];
        assert!(validate_argv(&argv).is_ok());
    }

    #[test]
    fn test_validate_argv_unsafe_control() {
        let argv = vec![
            "copilot".to_string(),
            "run".to_string(),
            "-p\n".to_string(),
            "some query".to_string(),
        ];
        assert!(validate_argv(&argv).is_err());
    }

    #[test]
    fn test_validate_argv_unsafe_injection() {
        let argv = vec![
            "copilot".to_string(),
            "run;".to_string(),
        ];
        assert!(validate_argv(&argv).is_err());
    }

    #[test]
    fn test_merge_tool_permissions() {
        let user_allow = vec!["read".to_string()];
        let user_deny = vec!["custom_deny".to_string()];
        let (allow, deny) = merge_tool_permissions(&user_allow, &user_deny, false);
        assert!(allow.contains(&"read".to_string()));
        assert!(deny.contains(&"custom_deny".to_string()));
        assert!(deny.contains(&"write(.env*)".to_string()));
    }
}

