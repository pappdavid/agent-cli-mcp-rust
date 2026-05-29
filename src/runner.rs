use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;
use chrono::Utc;

use crate::errors::{AgentCliError, ErrorCode};
use crate::redaction::redact_strict;
use crate::store::generate_session_id;

#[derive(Debug, Clone)]
pub struct SpawnResult {
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

pub struct SpawnOptions {
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    pub stdout_log_path: String,
    pub stderr_log_path: String,
    pub stdin_data: Option<String>,
}

pub async fn spawn_process(
    binary: &str,
    args: &[String],
    options: SpawnOptions,
) -> Result<SpawnResult, AgentCliError> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .current_dir(&options.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in &options.env {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == io::ErrorKind::NotFound {
                return Err(AgentCliError::new(
                    ErrorCode::BinaryNotFound,
                    &format!("Binary not found: {}", binary),
                ));
            } else {
                return Err(AgentCliError::new(
                    ErrorCode::BackendFailed,
                    &format!("Failed to spawn {}: {}", binary, e),
                ));
            }
        }
    };

    let mut stdin = child.stdin.take();
    if let Some(data) = options.stdin_data {
        if let Some(mut stdin_ch) = stdin {
            let _ = stdin_ch.write_all(data.as_bytes()).await;
            let _ = stdin_ch.flush().await;
        }
    }

    let stdout_log = options.stdout_log_path.clone();
    let stderr_log = options.stderr_log_path.clone();

    let mut stdout_stream = child.stdout.take().ok_or_else(|| {
        AgentCliError::new(ErrorCode::BackendFailed, "Failed to capture stdout stream")
    })?;
    let mut stderr_stream = child.stderr.take().ok_or_else(|| {
        AgentCliError::new(ErrorCode::BackendFailed, "Failed to capture stderr stream")
    })?;

    // Asynchronously write stdout/stderr to log files and accumulate into strings
    let stdout_handle = tokio::spawn(async move {
        let mut buffer = vec![0; 4096];
        let mut stdout_accum = String::new();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stdout_log)
            .ok();

        while let Ok(n) = stdout_stream.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            let s = String::from_utf8_lossy(&buffer[..n]);
            stdout_accum.push_str(&s);
            if let Some(ref mut f) = file {
                let _ = f.write_all(s.as_bytes());
            }
        }
        stdout_accum
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buffer = vec![0; 4096];
        let mut stderr_accum = String::new();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_log)
            .ok();

        while let Ok(n) = stderr_stream.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            let s = String::from_utf8_lossy(&buffer[..n]);
            stderr_accum.push_str(&s);
            if let Some(ref mut f) = file {
                let _ = f.write_all(s.as_bytes());
            }
        }
        stderr_accum
    });

    let exit_status = if let Some(timeout_ms) = options.timeout_ms {
        match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(e)) => Err(AgentCliError::new(
                ErrorCode::BackendFailed,
                &format!("Error waiting for process: {}", e),
            )),
            Err(_) => {
                // Timeout exceeded
                let _ = child.kill().await;
                return Err(AgentCliError::new(
                    ErrorCode::RunTimeout,
                    &format!("Process timed out after {}ms", timeout_ms),
                ));
            }
        }
    } else {
        match child.wait().await {
            Ok(status) => Ok(status),
            Err(e) => Err(AgentCliError::new(
                ErrorCode::BackendFailed,
                &format!("Error waiting for process: {}", e),
            )),
        }
    }?;

    let stdout = stdout_handle.await.unwrap_or_default();
    let stderr = stderr_handle.await.unwrap_or_default();

    Ok(SpawnResult {
        exit_code: exit_status.code(),
        signal: None,
        timed_out: false,
        stdout,
        stderr,
    })
}

pub async fn spawn_for_capability(
    binary: &str,
    args: &[String],
    timeout_ms: u64,
) -> Result<(bool, String), AgentCliError> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return Ok((false, String::new())),
    };

    let mut stdout_stream = child.stdout.take().unwrap();
    let mut stderr_stream = child.stderr.take().unwrap();

    let stdout_handle = tokio::spawn(async move {
        let mut accum = String::new();
        let mut buf = vec![0; 2048];
        while let Ok(n) = stdout_stream.read(&mut buf).await {
            if n == 0 {
                break;
            }
            accum.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        accum
    });

    let stderr_handle = tokio::spawn(async move {
        let mut accum = String::new();
        let mut buf = vec![0; 2048];
        while let Ok(n) = stderr_stream.read(&mut buf).await {
            if n == 0 {
                break;
            }
            accum.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        accum
    });

    match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = stdout_handle.await.unwrap_or_default();
            let stderr = stderr_handle.await.unwrap_or_default();
            let output = format!("{}{}", stdout, stderr);
            Ok((status.success() || !output.is_empty(), redact_strict(&output)))
        }
        _ => {
            let _ = child.kill().await;
            Ok((false, String::new()))
        }
    }
}

// ── Interactive sessions ─────────────────────────────────────────────────────

pub struct ActiveSession {
    pub id: String,
    pub service: String,
    pub cwd: String,
    pub stdout_log_path: String,
    pub stderr_log_path: String,
    pub status: String, // "running" | "complete" | "failed" | "killed"
    pub started_at: String,
    pub run_id: Option<String>,
    pub stdin_tx: Option<Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>>,
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Arc<RwLock<ActiveSession>>>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start(
        &self,
        binary: &str,
        args: &[String],
        opts: SpawnOptions,
        run_id: Option<String>,
        service: &str,
    ) -> Result<String, AgentCliError> {
        let id = generate_session_id();
        let mut cmd = Command::new(binary);
        cmd.args(args)
            .current_dir(&opts.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    return Err(AgentCliError::new(
                        ErrorCode::BinaryNotFound,
                        &format!("Binary not found: {}", binary),
                    ));
                } else {
                    return Err(AgentCliError::new(
                        ErrorCode::BackendFailed,
                        &format!("Failed to spawn {}: {}", binary, e),
                    ));
                }
            }
        };

        let stdin_tx = child.stdin.take().unwrap();
        let mut stdout_stream = child.stdout.take().unwrap();
        let mut stderr_stream = child.stderr.take().unwrap();

        let stdout_log = opts.stdout_log_path.clone();
        let stderr_log = opts.stderr_log_path.clone();

        let stdout_task = tokio::spawn(async move {
            let mut buf = vec![0; 4096];
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&stdout_log)
                .ok();

            while let Ok(n) = stdout_stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Some(ref mut f) = file {
                    let _ = f.write_all(&buf[..n]);
                }
            }
        });

        let stderr_task = tokio::spawn(async move {
            let mut buf = vec![0; 4096];
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&stderr_log)
                .ok();

            while let Ok(n) = stderr_stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Some(ref mut f) = file {
                    let _ = f.write_all(&buf[..n]);
                }
            }
        });

        let session = Arc::new(RwLock::new(ActiveSession {
            id: id.clone(),
            service: service.to_string(),
            cwd: opts.cwd,
            stdout_log_path: opts.stdout_log_path,
            stderr_log_path: opts.stderr_log_path,
            status: "running".to_string(),
            started_at: Utc::now().to_rfc3339(),
            run_id,
            stdin_tx: Some(Arc::new(tokio::sync::Mutex::new(stdin_tx))),
        }));

        self.sessions.write().unwrap().insert(id.clone(), session.clone());

        let sessions_clone = self.sessions.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            let code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(0),
                Err(_) => -1,
            };

            let _ = stdout_task.await;
            let _ = stderr_task.await;

            if let Some(sess_lock) = sessions_clone.read().unwrap().get(&id_clone) {
                let mut s = sess_lock.write().unwrap();
                s.status = if code == 0 { "complete".to_string() } else { "failed".to_string() };
                s.stdin_tx = None;
            }
        });

        Ok(id)
    }

    pub fn get_session_info(&self, id: &str) -> Option<SessionStateInfo> {
        let sess_lock = self.sessions.read().unwrap().get(id)?.clone();
        let s = sess_lock.read().unwrap();
        Some(SessionStateInfo {
            id: s.id.clone(),
            service: s.service.clone(),
            cwd: s.cwd.clone(),
            stdout_log_path: s.stdout_log_path.clone(),
            stderr_log_path: s.stderr_log_path.clone(),
            status: s.status.clone(),
            started_at: s.started_at.clone(),
            run_id: s.run_id.clone(),
        })
    }

    pub fn list(&self) -> Vec<SessionStateInfo> {
        let s_map = self.sessions.read().unwrap();
        s_map
            .values()
            .map(|sess_lock| {
                let s = sess_lock.read().unwrap();
                SessionStateInfo {
                    id: s.id.clone(),
                    service: s.service.clone(),
                    cwd: s.cwd.clone(),
                    stdout_log_path: s.stdout_log_path.clone(),
                    stderr_log_path: s.stderr_log_path.clone(),
                    status: s.status.clone(),
                    started_at: s.started_at.clone(),
                    run_id: s.run_id.clone(),
                }
            })
            .collect()
    }

    pub async fn send_input(&self, id: &str, input: &str) -> Result<(), AgentCliError> {
        let sess_lock = self.sessions.read().unwrap().get(id).cloned().ok_or_else(|| {
            AgentCliError::new(ErrorCode::SessionNotFound, &format!("Session not found: {}", id))
        })?;

        let stdin_tx_opt = {
            let s = sess_lock.read().unwrap();
            if s.status != "running" {
                return Err(AgentCliError::new(
                    ErrorCode::SessionNotFound,
                    &format!("Session {} is not running (status: {})", id, s.status),
                ));
            }
            s.stdin_tx.clone()
        };

        if let Some(stdin_tx) = stdin_tx_opt {
            let mut tx = stdin_tx.lock().await;
            if let Err(e) = tx.write_all(input.as_bytes()).await {
                return Err(AgentCliError::new(
                    ErrorCode::BackendFailed,
                    &format!("Failed to write to session stdin: {}", e),
                ));
            }
            let _ = tx.flush().await;
            Ok(())
        } else {
            Err(AgentCliError::new(
                ErrorCode::BackendFailed,
                "Stdin stream not available for session",
            ))
        }
    }

    pub async fn kill(&self, id: &str, reason: Option<&str>) -> Result<(), AgentCliError> {
        let sess_lock = self.sessions.read().unwrap().get(id).cloned().ok_or_else(|| {
            AgentCliError::new(ErrorCode::SessionNotFound, &format!("Session not found: {}", id))
        })?;

        let mut s = sess_lock.write().unwrap();
        s.status = "killed".to_string();
        s.stdin_tx = None; // Dropping ChildStdin will send EOF and trigger termination in many interactive CLIs

        if let Some(r) = reason {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&s.stderr_log_path)
                .ok();
            if let Some(ref mut f) = file {
                let _ = writeln!(f, "\n[KILLED: {}]", r);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SessionStateInfo {
    pub id: String,
    pub service: String,
    pub cwd: String,
    pub stdout_log_path: String,
    pub stderr_log_path: String,
    pub status: String,
    pub started_at: String,
    pub run_id: Option<String>,
}
