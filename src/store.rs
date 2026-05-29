use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use rand::Rng;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: String,
    pub service: String, // "copilot" | "jules"
    pub mode: String,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub cwd: String,
    pub argv: Vec<String>,
    pub status: String, // "running" | "complete" | "failed" | "killed" | "quarantined"
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "endedAt")]
    pub ended_at: Option<String>,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    #[serde(rename = "stdoutLog")]
    pub stdout_log: String,
    #[serde(rename = "stderrLog")]
    pub stderr_log: String,
    #[serde(rename = "promptFile")]
    pub prompt_file: Option<String>,
    pub worktree: Option<String>,
    #[serde(rename = "remoteSessionId")]
    pub remote_session_id: Option<String>,
    #[serde(rename = "remoteSource")]
    pub remote_source: Option<String>,
    #[serde(rename = "remoteUrl")]
    pub remote_url: Option<String>,
    #[serde(rename = "remoteState")]
    pub remote_state: Option<String>,
    #[serde(rename = "normalizedStatus")]
    pub normalized_status: Option<String>,
    #[serde(rename = "lastActivityTime")]
    pub last_activity_time: Option<String>,
    #[serde(rename = "lastReconciledAt")]
    pub last_reconciled_at: Option<String>,
    #[serde(rename = "staleReason")]
    pub stale_reason: Option<String>,
    #[serde(rename = "sanitizedCommandSummary")]
    pub sanitized_command_summary: String,
}

pub struct Store {
    pub runs_file: PathBuf,
    pub logs_dir: PathBuf,
}

impl Store {
    pub fn new<P: AsRef<Path>>(state_dir: P) -> io::Result<Self> {
        let state_dir_ref = state_dir.as_ref();
        fs::create_dir_all(state_dir_ref)?;
        let runs_file = state_dir_ref.join("runs.jsonl");
        let logs_dir = state_dir_ref.join("logs");
        fs::create_dir_all(&logs_dir)?;

        Ok(Store {
            runs_file,
            logs_dir,
        })
    }

    pub fn allocate_log_paths(&self, run_id: &str) -> io::Result<(String, String)> {
        let dir = self.logs_dir.join(run_id);
        fs::create_dir_all(&dir)?;
        Ok((
            dir.join("stdout.log").to_string_lossy().to_string(),
            dir.join("stderr.log").to_string_lossy().to_string(),
        ))
    }

    pub fn save(&self, run: &AgentRun) -> io::Result<()> {
        let serialized = serde_json::to_string(run)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.runs_file)?;
        writeln!(file, "{}", serialized)?;
        Ok(())
    }

    pub fn all(&self) -> Vec<AgentRun> {
        if !self.runs_file.exists() {
            return Vec::new();
        }
        let content = match fs::read_to_string(&self.runs_file) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<AgentRun>(l).ok())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<AgentRun> {
        self.all().into_iter().find(|r| r.id == id)
    }

    pub fn update(&self, id: &str, patch: AgentRunPatch) -> io::Result<()> {
        let mut runs = self.all();
        if let Some(idx) = runs.iter().rposition(|r| r.id == id) {
            let run = &mut runs[idx];
            if let Some(s) = patch.status { run.status = s; }
            if let Some(e) = patch.ended_at { run.ended_at = Some(e); }
            if let Some(ec) = patch.exit_code { run.exit_code = ec; }
            if let Some(rs) = patch.remote_session_id { run.remote_session_id = Some(rs); }
            if let Some(rsrc) = patch.remote_source { run.remote_source = Some(rsrc); }
            if let Some(ru) = patch.remote_url { run.remote_url = Some(ru); }
            if let Some(rst) = patch.remote_state { run.remote_state = Some(rst); }
            if let Some(ns) = patch.normalized_status { run.normalized_status = Some(ns); }
            if let Some(la) = patch.last_activity_time { run.last_activity_time = Some(la); }
            if let Some(lr) = patch.last_reconciled_at { run.last_reconciled_at = Some(lr); }
            if let Some(sr) = patch.stale_reason { run.stale_reason = Some(sr); }

            let mut file = fs::File::create(&self.runs_file)?;
            for r in runs {
                let serialized = serde_json::to_string(&r)?;
                writeln!(file, "{}", serialized)?;
            }
        }
        Ok(())
    }

    pub fn list(&self, service: &str, status: &str, limit: Option<usize>) -> Vec<AgentRun> {
        let mut runs = self.all();
        runs.reverse(); // Newest first

        let mut filtered: Vec<AgentRun> = runs
            .into_iter()
            .filter(|r| service == "all" || r.service == service)
            .filter(|r| status == "all" || r.status == status)
            .collect();

        if let Some(lim) = limit {
            filtered.truncate(lim);
        } else {
            filtered.truncate(50);
        }
        filtered
    }

    pub fn prune_older_than(&self, days: i64) -> io::Result<()> {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        let runs = self.all();
        let kept: Vec<AgentRun> = runs
            .into_iter()
            .filter(|r| {
                if let Ok(parsed) = DateTime::parse_from_rfc3339(&r.started_at) {
                    parsed.with_timezone(&Utc) >= cutoff
                } else {
                    true // If we can't parse, keep it to be safe
                }
            })
            .collect();

        let mut file = fs::File::create(&self.runs_file)?;
        for r in kept {
            let serialized = serde_json::to_string(&r)?;
            writeln!(file, "{}", serialized)?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct AgentRunPatch {
    pub status: Option<String>,
    pub ended_at: Option<String>,
    pub exit_code: Option<Option<i32>>,
    pub remote_session_id: Option<String>,
    pub remote_source: Option<String>,
    pub remote_url: Option<String>,
    pub remote_state: Option<String>,
    pub normalized_status: Option<String>,
    pub last_activity_time: Option<String>,
    pub last_reconciled_at: Option<String>,
    pub stale_reason: Option<String>,
}

fn to_base36(mut val: u64) -> String {
    if val == 0 {
        return "0".to_string();
    }
    let chars = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut res = Vec::new();
    while val > 0 {
        let rem = (val % 36) as usize;
        res.push(chars[rem]);
        val /= 36;
    }
    res.reverse();
    String::from_utf8(res).unwrap_or_default()
}

pub fn generate_run_id() -> String {
    let ts = Utc::now().timestamp_millis() as u64;
    let ts_36 = to_base36(ts);
    let rand_val: u32 = rand::thread_rng().gen_range(100_000..999_999);
    let rand_36 = to_base36(rand_val as u64);
    format!("run_{}_{}", ts_36, rand_36)
}

pub fn generate_session_id() -> String {
    let ts = Utc::now().timestamp_millis() as u64;
    let ts_36 = to_base36(ts);
    let rand_val: u32 = rand::thread_rng().gen_range(100_000..999_999);
    let rand_36 = to_base36(rand_val as u64);
    format!("sess_{}_{}", ts_36, rand_36)
}
