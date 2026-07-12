use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileConfig {
    pub allowed_roots: Option<Vec<String>>,
    pub state_dir: Option<String>,
    pub default_timeout_ms: Option<u64>,
    pub max_output_chars: Option<usize>,
    pub copilot_bin: Option<String>,
    pub jules_bin: Option<String>,
    pub gh_bin: Option<String>,
    pub gemini_bin: Option<String>,
    pub codex_bin: Option<String>,
    pub opencode_bin: Option<String>,
    pub claude_bin: Option<String>,
    pub worktree_root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub allowed_roots: Vec<String>,
    pub state_dir: String,
    pub default_timeout_ms: u64,
    pub max_output_chars: usize,
    pub copilot_bin: String,
    pub jules_bin: String,
    pub gh_bin: String,
    pub gemini_bin: String,
    pub codex_bin: String,
    pub opencode_bin: String,
    pub claude_bin: String,
    pub jules_api_key: Option<String>,
    pub jules_api_key_cmd_parts: Option<Vec<String>>,
    pub github_token_cmd_parts: Option<Vec<String>>,
    pub worktree_root: Option<String>,
}

fn parse_allowed_roots(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .map(canonicalize_path)
        .collect()
}

fn parse_cmd_parts(raw: Option<String>) -> Option<Vec<String>> {
    let s = raw?;
    let parts: Vec<String> = s.split_whitespace().map(|p| p.to_string()).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

pub fn canonicalize_path(p: &str) -> String {
    let expanded = if p.starts_with("~/") {
        if let Some(home) = env::var_os("HOME") {
            let mut path = PathBuf::from(home);
            path.push(&p[2..]);
            path
        } else {
            PathBuf::from(p)
        }
    } else {
        PathBuf::from(p)
    };

    if let Ok(abs) = fs::canonicalize(&expanded) {
        abs.to_string_lossy().to_string()
    } else {
        // If it doesn't exist yet, we still resolve absolute path as best we can
        if let Ok(abs) = std::path::absolute(&expanded) {
            abs.to_string_lossy().to_string()
        } else {
            expanded.to_string_lossy().to_string()
        }
    }
}

pub fn load_config() -> Config {
    let mut file_config = FileConfig {
        allowed_roots: None,
        state_dir: None,
        default_timeout_ms: None,
        max_output_chars: None,
        copilot_bin: None,
        jules_bin: None,
        gh_bin: None,
        gemini_bin: None,
        codex_bin: None,
        opencode_bin: None,
        claude_bin: None,
        worktree_root: None,
    };

    if let Ok(config_path_str) = env::var("AGENT_CLI_CONFIG") {
        let config_path = Path::new(&config_path_str);
        if config_path.exists() {
            if let Ok(content) = fs::read_to_string(config_path) {
                if let Ok(parsed) = serde_json::from_str::<FileConfig>(&content) {
                    file_config = parsed;
                }
            }
        }
    }

    let mut allowed_roots = Vec::new();
    if let Ok(roots_env) = env::var("AGENT_CLI_ALLOWED_ROOTS") {
        allowed_roots = parse_allowed_roots(&roots_env);
    } else if let Some(roots) = file_config.allowed_roots {
        allowed_roots = roots.iter().map(|r| canonicalize_path(r)).collect();
    }

    if allowed_roots.is_empty() {
        // Sensible defaults
        if let Some(home) = env::var_os("HOME") {
            let home_path = PathBuf::from(home);
            allowed_roots.push(canonicalize_path(&home_path.join("Dev").to_string_lossy()));
            allowed_roots.push(canonicalize_path(&home_path.join("dev").to_string_lossy()));
            allowed_roots.push(canonicalize_path(
                &home_path.join("projects").to_string_lossy(),
            ));
            allowed_roots.push(canonicalize_path(
                &home_path.join(".codex/worktrees").to_string_lossy(),
            ));
            allowed_roots.push(canonicalize_path(
                &home_path.join(".claude/worktrees").to_string_lossy(),
            ));
        }
    }

    let state_dir = env::var("AGENT_CLI_STATE_DIR")
        .ok()
        .or(file_config.state_dir)
        .unwrap_or_else(|| {
            if let Some(home) = env::var_os("HOME") {
                PathBuf::from(home)
                    .join(".agent-cli-mcp")
                    .to_string_lossy()
                    .to_string()
            } else {
                ".agent-cli-mcp".to_string()
            }
        });

    let default_timeout_ms = env::var("AGENT_CLI_DEFAULT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or(file_config.default_timeout_ms)
        .unwrap_or(1_800_000);

    let max_output_chars = env::var("AGENT_CLI_MAX_OUTPUT_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or(file_config.max_output_chars)
        .unwrap_or(50_000);

    let copilot_bin = env::var("COPILOT_BIN")
        .ok()
        .or(file_config.copilot_bin)
        .unwrap_or_else(|| "copilot".to_string());

    let jules_bin = env::var("JULES_BIN")
        .ok()
        .or(file_config.jules_bin)
        .unwrap_or_else(|| "jules".to_string());

    let gh_bin = env::var("GH_BIN")
        .ok()
        .or(file_config.gh_bin)
        .unwrap_or_else(|| "gh".to_string());

    let gemini_bin = env::var("GEMINI_BIN")
        .ok()
        .or(file_config.gemini_bin)
        .unwrap_or_else(|| "gemini".to_string());

    let codex_bin = env::var("CODEX_BIN")
        .ok()
        .or(file_config.codex_bin)
        .unwrap_or_else(|| "codex".to_string());

    let opencode_bin = env::var("OPENCODE_BIN")
        .ok()
        .or(file_config.opencode_bin)
        .unwrap_or_else(|| "opencode".to_string());

    let claude_bin = env::var("CLAUDE_BIN")
        .ok()
        .or(file_config.claude_bin)
        .unwrap_or_else(|| "claude".to_string());

    let jules_api_key = env::var("JULES_API_KEY").ok();
    let jules_api_key_cmd_parts = parse_cmd_parts(env::var("JULES_API_KEY_CMD").ok());
    let github_token_cmd_parts = parse_cmd_parts(env::var("GITHUB_TOKEN_CMD").ok());

    let worktree_root = env::var("AGENT_CLI_WORKTREE_ROOT")
        .ok()
        .or(file_config.worktree_root);

    Config {
        allowed_roots,
        state_dir: canonicalize_path(&state_dir),
        default_timeout_ms,
        max_output_chars,
        copilot_bin,
        jules_bin,
        gh_bin,
        gemini_bin,
        codex_bin,
        opencode_bin,
        claude_bin,
        jules_api_key,
        jules_api_key_cmd_parts,
        github_token_cmd_parts,
        worktree_root,
    }
}
