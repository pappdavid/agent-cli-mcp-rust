use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::process::Command;
use crate::config::Config;
use crate::errors::{AgentCliError, ErrorCode};
use crate::jules_status::{JulesActivity, JulesSession, SourceContext};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateJulesSessionRequest {
    pub prompt: String,
    pub source_context: SourceContext,
    pub title: Option<String>,
    pub require_plan_approval: Option<bool>,
    pub automation_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesSource {
    pub name: Option<String>,
    pub id: Option<String>,
    pub github_repo: Option<GithubRepoInfo>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoInfo {
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub full_name: Option<String>,
    pub html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListSessionsResponse {
    sessions: Option<Vec<JulesSession>>,
}

#[derive(Debug, Deserialize)]
struct ListActivitiesResponse {
    activities: Option<Vec<JulesActivity>>,
}

#[derive(Debug, Deserialize)]
struct ListSourcesResponse {
    sources: Option<Vec<JulesSource>>,
}

pub struct JulesClient {
    client: Client,
    api_key: String,
}

const API_BASE_URL: &str = "https://jules.googleapis.com/v1alpha";

fn get_api_key(config: &Config) -> Result<String, AgentCliError> {
    if let Some(ref key) = config.jules_api_key {
        return Ok(key.clone());
    }

    if let Some(ref cmd_parts) = config.jules_api_key_cmd_parts {
        if cmd_parts.is_empty() {
            return Err(AgentCliError::new(
                ErrorCode::UnsupportedCapability,
                "Jules API support requires JULES_API_KEY or JULES_API_KEY_CMD to be configured.",
            ));
        }

        let output = Command::new(&cmd_parts[0])
            .args(&cmd_parts[1..])
            .output()
            .map_err(|e| {
                AgentCliError::new(
                    ErrorCode::BackendFailed,
                    &format!("Failed to retrieve Jules API key via JULES_API_KEY_CMD: {}", e),
                )
            })?;

        if !output.status.success() {
            return Err(AgentCliError::new(
                ErrorCode::BackendFailed,
                "Jules API key command exited with failure status.",
            ));
        }

        let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if key.is_empty() {
            return Err(AgentCliError::new(
                ErrorCode::BackendFailed,
                "Jules API key command returned an empty result.",
            ));
        }
        return Ok(key);
    }

    Err(AgentCliError::new(
        ErrorCode::UnsupportedCapability,
        "Jules API support requires JULES_API_KEY or JULES_API_KEY_CMD to be configured.",
    ))
}

fn resource_name(session_id: &str) -> String {
    if session_id.starts_with("sessions/") {
        session_id.to_string()
    } else {
        format!("sessions/{}", session_id)
    }
}

impl JulesClient {
    pub fn new(config: &Config) -> Result<Self, AgentCliError> {
        let api_key = get_api_key(config)?;
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AgentCliError::new(ErrorCode::BackendFailed, &format!("Failed to build HTTP client: {}", e)))?;

        Ok(JulesClient { client, api_key })
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        pathname: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, AgentCliError> {
        let url = format!("{}{}", API_BASE_URL, pathname);
        let mut builder = self.client.request(method.clone(), &url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json");

        if let Some(b) = body {
            builder = builder.json(&b);
        }

        let response = builder.send().await
            .map_err(|e| AgentCliError::new(ErrorCode::BackendFailed, &format!("Jules API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AgentCliError::new(
                ErrorCode::BackendFailed,
                &format!("Jules API request {} failed with {}: {}", pathname, status, text),
            ));
        }

        let text = response.text().await
            .map_err(|e| AgentCliError::new(ErrorCode::BackendFailed, &format!("Failed to read response body: {}", e)))?;

        if text.trim().is_empty() {
            // Return empty value or fallback representation
            return serde_json::from_str("null")
                .map_err(|e| AgentCliError::new(ErrorCode::BackendFailed, &format!("Failed to deserialize: {}", e)));
        }

        serde_json::from_str(&text)
            .map_err(|e| AgentCliError::new(ErrorCode::BackendFailed, &format!("Failed to deserialize: {}\nBody: {}", e, text)))
    }

    pub async fn create_session(&self, req: CreateJulesSessionRequest) -> Result<JulesSession, AgentCliError> {
        let body = serde_json::to_value(&req).unwrap();
        self.request(reqwest::Method::POST, "/sessions", Some(body)).await
    }

    pub async fn list_sessions(&self, page_size: i32, page_token: Option<&str>) -> Result<Vec<JulesSession>, AgentCliError> {
        let mut pathname = format!("/sessions?pageSize={}", page_size);
        if let Some(tok) = page_token {
            pathname.push_str(&format!("&pageToken={}", tok));
        }
        let res: ListSessionsResponse = self.request(reqwest::Method::GET, &pathname, None).await?;
        Ok(res.sessions.unwrap_or_default())
    }

    pub async fn get_session(&self, session_id: &str) -> Result<JulesSession, AgentCliError> {
        let pathname = format!("/{}", resource_name(session_id));
        self.request(reqwest::Method::GET, &pathname, None).await
    }

    pub async fn list_activities(
        &self,
        session_id: &str,
        page_size: i32,
        page_token: Option<&str>,
    ) -> Result<Vec<JulesActivity>, AgentCliError> {
        let mut pathname = format!("/{}/activities?pageSize={}", resource_name(session_id), page_size);
        if let Some(tok) = page_token {
            pathname.push_str(&format!("&pageToken={}", tok));
        }
        let res: ListActivitiesResponse = self.request(reqwest::Method::GET, &pathname, None).await?;
        Ok(res.activities.unwrap_or_default())
    }

    pub async fn approve_plan(&self, session_id: &str) -> Result<(), AgentCliError> {
        let pathname = format!("/{}:approvePlan", resource_name(session_id));
        let _: serde_json::Value = self.request(reqwest::Method::POST, &pathname, Some(serde_json::Value::Object(serde_json::Map::new()))).await?;
        Ok(())
    }

    pub async fn send_message(&self, session_id: &str, prompt: &str) -> Result<(), AgentCliError> {
        let pathname = format!("/{}:sendMessage", resource_name(session_id));
        let mut body = serde_json::Map::new();
        body.insert("prompt".to_string(), serde_json::Value::String(prompt.to_string()));
        let _: serde_json::Value = self.request(reqwest::Method::POST, &pathname, Some(serde_json::Value::Object(body))).await?;
        Ok(())
    }

    pub async fn list_sources(&self, page_size: i32, page_token: Option<&str>) -> Result<Vec<JulesSource>, AgentCliError> {
        let mut pathname = format!("/sources?pageSize={}", page_size);
        if let Some(tok) = page_token {
            pathname.push_str(&format!("&pageToken={}", tok));
        }
        let res: ListSourcesResponse = self.request(reqwest::Method::GET, &pathname, None).await?;
        Ok(res.sources.unwrap_or_default())
    }
}
