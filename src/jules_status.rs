use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedJulesStatus {
    NotCreated,
    Queued,
    Planning,
    AwaitingPlanApproval,
    AwaitingUserFeedback,
    InProgress,
    Paused,
    Completed,
    Failed,
    StaleLocalRun,
    Unknown,
}

impl NormalizedJulesStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            NormalizedJulesStatus::NotCreated => "not_created",
            NormalizedJulesStatus::Queued => "queued",
            NormalizedJulesStatus::Planning => "planning",
            NormalizedJulesStatus::AwaitingPlanApproval => "awaiting_plan_approval",
            NormalizedJulesStatus::AwaitingUserFeedback => "awaiting_user_feedback",
            NormalizedJulesStatus::InProgress => "in_progress",
            NormalizedJulesStatus::Paused => "paused",
            NormalizedJulesStatus::Completed => "completed",
            NormalizedJulesStatus::Failed => "failed",
            NormalizedJulesStatus::StaleLocalRun => "stale_local_run",
            NormalizedJulesStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JulesPlanStep {
    pub id: Option<String>,
    pub index: Option<i32>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesPlan {
    pub id: Option<String>,
    pub create_time: Option<String>,
    pub steps: Option<Vec<JulesPlanStep>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesActivity {
    pub id: Option<String>,
    pub name: Option<String>,
    pub originator: Option<String>,
    pub description: Option<String>,
    pub create_time: Option<String>,
    pub plan_generated: Option<PlanGenerated>,
    pub plan_approved: Option<PlanApproved>,
    pub user_messaged: Option<UserMessaged>,
    pub agent_messaged: Option<AgentMessaged>,
    pub progress_updated: Option<ProgressUpdated>,
    pub session_completed: Option<serde_json::Value>,
    pub session_failed: Option<SessionFailed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanGenerated {
    pub plan: Option<JulesPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApproved {
    pub plan_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessaged {
    pub user_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessaged {
    pub agent_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUpdated {
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFailed {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JulesPullRequest {
    pub url: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesSessionOutput {
    pub pull_request: Option<JulesPullRequest>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesSession {
    pub id: Option<String>,
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub title: Option<String>,
    pub state: Option<String>,
    pub url: Option<String>,
    pub source_context: Option<SourceContext>,
    pub outputs: Option<Vec<JulesSessionOutput>>,
    pub create_time: Option<String>,
    pub update_time: Option<String>,
    pub require_plan_approval: Option<bool>,
    pub automation_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceContext {
    pub source: Option<String>,
    pub github_repo_context: Option<GithubRepoContext>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoContext {
    pub starting_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesStatusSummary {
    pub ok: bool,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub state: Option<String>,
    pub normalized_status: NormalizedJulesStatus,
    pub url: Option<String>,
    pub source: Option<String>,
    pub latest_activity_time: Option<String>,
    pub needs_action: bool,
    pub needed_action: Option<String>,
    pub plan: Option<JulesPlan>,
    pub progress: Vec<String>,
    pub outputs: Vec<JulesSessionOutput>,
    pub pr: Option<JulesPullRequest>,
    pub latest_agent_message: Option<String>,
    pub latest_user_message: Option<String>,
    pub failure_message: Option<String>,
    pub stale_reason: Option<String>,
}

pub fn normalize_jules_session_state(state: Option<&str>) -> NormalizedJulesStatus {
    match state {
        Some("QUEUED") => NormalizedJulesStatus::Queued,
        Some("PLANNING") => NormalizedJulesStatus::Planning,
        Some("AWAITING_PLAN_APPROVAL") => NormalizedJulesStatus::AwaitingPlanApproval,
        Some("AWAITING_USER_FEEDBACK") => NormalizedJulesStatus::AwaitingUserFeedback,
        Some("IN_PROGRESS") => NormalizedJulesStatus::InProgress,
        Some("PAUSED") => NormalizedJulesStatus::Paused,
        Some("COMPLETED") => NormalizedJulesStatus::Completed,
        Some("FAILED") => NormalizedJulesStatus::Failed,
        Some("STATE_UNSPECIFIED") => NormalizedJulesStatus::Unknown,
        None => NormalizedJulesStatus::NotCreated,
        _ => NormalizedJulesStatus::Unknown,
    }
}

pub fn is_active_normalized_status(status: &NormalizedJulesStatus) -> bool {
    matches!(
        status,
        NormalizedJulesStatus::Queued
            | NormalizedJulesStatus::Planning
            | NormalizedJulesStatus::AwaitingPlanApproval
            | NormalizedJulesStatus::AwaitingUserFeedback
            | NormalizedJulesStatus::InProgress
            | NormalizedJulesStatus::Paused
    )
}

fn describe_activity(activity: &JulesActivity) -> Option<String> {
    if let Some(ref d) = activity.description {
        return Some(d.clone());
    }
    if let Some(ref pg) = activity.plan_generated {
        if pg.plan.is_some() {
            return Some("Plan generated".to_string());
        }
    }
    if let Some(ref pa) = activity.plan_approved {
        if let Some(ref pid) = pa.plan_id {
            return Some(format!("Plan approved ({})", pid));
        }
    }
    if let Some(ref um) = activity.user_messaged {
        if let Some(ref msg) = um.user_message {
            return Some(format!("User: {}", msg));
        }
    }
    if let Some(ref am) = activity.agent_messaged {
        if let Some(ref msg) = am.agent_message {
            return Some(msg.clone());
        }
    }
    if let Some(ref pu) = activity.progress_updated {
        match (&pu.title, &pu.description) {
            (Some(t), Some(d)) => return Some(format!("{}: {}", t, d)),
            (Some(t), None) => return Some(t.clone()),
            (None, Some(d)) => return Some(d.clone()),
            _ => {}
        }
    }
    if activity.session_completed.is_some() {
        return Some("Session completed".to_string());
    }
    if let Some(ref sf) = activity.session_failed {
        if let Some(ref r) = sf.reason {
            return Some(format!("Session failed: {}", r));
        }
    }
    None
}

pub struct BuildJulesStatusInput {
    pub session: Option<JulesSession>,
    pub activities: Vec<JulesActivity>,
    pub run_id: Option<String>,
    pub local_normalized_status: Option<String>,
    pub local_stale_reason: Option<String>,
    pub local_last_activity_time: Option<String>,
}

pub fn build_jules_status(input: BuildJulesStatusInput) -> JulesStatusSummary {
    let mut ordered_activities = input.activities.clone();
    ordered_activities.sort_by(|a, b| {
        let at = a
            .create_time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt: chrono::DateTime<chrono::FixedOffset>| dt.timestamp_millis())
            .unwrap_or(0);
        let bt = b
            .create_time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt: chrono::DateTime<chrono::FixedOffset>| dt.timestamp_millis())
            .unwrap_or(0);
        at.cmp(&bt)
    });

    let latest_activity = ordered_activities.last().cloned();
    let latest_plan = ordered_activities
        .iter()
        .rev()
        .find_map(|act| act.plan_generated.as_ref()?.plan.clone());
    let latest_agent_message = ordered_activities
        .iter()
        .rev()
        .find_map(|act| act.agent_messaged.as_ref()?.agent_message.clone());
    let latest_user_message = ordered_activities
        .iter()
        .rev()
        .find_map(|act| act.user_messaged.as_ref()?.user_message.clone());
    let latest_failure = ordered_activities
        .iter()
        .rev()
        .find_map(|act| act.session_failed.as_ref()?.reason.clone());

    let outputs = input.session.as_ref().and_then(|s| s.outputs.clone()).unwrap_or_default();
    let pr = outputs
        .iter()
        .find_map(|out| out.pull_request.clone());

    let normalized_status = if input.local_normalized_status.as_deref() == Some("stale_local_run") {
        NormalizedJulesStatus::StaleLocalRun
    } else {
        normalize_jules_session_state(input.session.as_ref().and_then(|s| s.state.as_deref()))
    };

    let needed_action = match normalized_status {
        NormalizedJulesStatus::AwaitingPlanApproval => Some("approve_plan".to_string()),
        NormalizedJulesStatus::AwaitingUserFeedback | NormalizedJulesStatus::Paused => Some("send_message".to_string()),
        NormalizedJulesStatus::Completed if pr.is_some() || !outputs.is_empty() => Some("verify_pr".to_string()),
        NormalizedJulesStatus::Failed => Some("inspect_failure".to_string()),
        NormalizedJulesStatus::StaleLocalRun => Some("reconcile_local_run".to_string()),
        _ => None,
    };

    let needs_action = needed_action.is_some();

    let latest_activity_time = latest_activity
        .as_ref()
        .and_then(|act| act.create_time.clone())
        .or_else(|| input.session.as_ref().and_then(|s| s.update_time.clone()))
        .or(input.local_last_activity_time);

    let progress = ordered_activities
        .iter()
        .filter_map(|act| describe_activity(act))
        .collect();

    JulesStatusSummary {
        ok: true,
        run_id: input.run_id,
        session_id: input
            .session
            .as_ref()
            .and_then(|s| s.id.clone().or_else(|| s.name.as_ref().map(|n| n.replace("sessions/", "")))),
        title: input.session.as_ref().and_then(|s| s.title.clone()),
        state: input.session.as_ref().and_then(|s| s.state.clone()),
        normalized_status,
        url: input.session.as_ref().and_then(|s| s.url.clone()),
        source: input
            .session
            .as_ref()
            .and_then(|s| s.source_context.as_ref()?.source.clone()),
        latest_activity_time,
        needs_action,
        needed_action,
        plan: latest_plan,
        progress,
        outputs,
        pr,
        latest_agent_message,
        latest_user_message,
        failure_message: latest_failure,
        stale_reason: input.local_stale_reason,
    }
}
