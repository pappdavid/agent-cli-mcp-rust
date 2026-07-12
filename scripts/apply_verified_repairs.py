#!/usr/bin/env python3
"""Apply reviewed CI-driven repairs and abort on any unexpected source shape."""

from __future__ import annotations

from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def remove_struct_fields(text: str, struct_name: str, fields: set[str]) -> str:
    marker = f"pub struct {struct_name} {{"
    start = text.index(marker)
    end = text.index("\n}", start)
    block = text[start:end]
    kept: list[str] = []
    removed: set[str] = set()
    for line in block.splitlines():
        field = next(
            (name for name in fields if line.strip().startswith(f"pub {name}:")),
            None,
        )
        if field is None:
            kept.append(line)
        else:
            removed.add(field)
    missing = fields - removed
    if missing:
        raise RuntimeError(f"{struct_name}: fields not found: {sorted(missing)}")
    updated = text[:start] + "\n".join(kept) + text[end:]
    return replace_once(
        updated,
        '#[serde(rename_all = "camelCase")]\n' + marker,
        '#[serde(rename_all = "camelCase", deny_unknown_fields)]\n' + marker,
        f"{struct_name} serde policy",
    )


def remove_schema_properties(text: str, marker: str, fields: set[str]) -> str:
    start = text.index(marker)
    next_section = text.find("\n    // ", start + len(marker))
    end = len(text) if next_section == -1 else next_section
    block = text[start:end]
    kept: list[str] = []
    removed: set[str] = set()
    for line in block.splitlines(keepends=True):
        field = next((name for name in fields if f'"{name}":' in line), None)
        if field is None:
            kept.append(line)
        else:
            removed.add(field)
    missing = fields - removed
    if missing:
        raise RuntimeError(f"{marker}: schema fields not found: {sorted(missing)}")
    return text[:start] + "".join(kept) + text[end:]


def repair_config() -> None:
    path = Path("src/config.rs")
    text = path.read_text()
    text = replace_once(
        text,
        'if p.starts_with("~/") {',
        'if let Some(stripped) = p.strip_prefix("~/") {',
        "safe home-prefix detection",
    )
    text = replace_once(
        text,
        "path.push(&p[2..]);",
        "path.push(stripped);",
        "safe home-prefix removal",
    )
    path.write_text(text)


def repair_policy() -> None:
    path = Path("src/policy.rs")
    text = path.read_text()
    text = replace_once(
        text,
        '''        let parent = path.parent();
        if path.file_name().and_then(|n| n.to_str()) == Some(".git") && parent.is_some() {
            canonicalize_path(&parent.unwrap().to_string_lossy())
        } else {
            repo_root_canonical.clone()
        }''',
        '''        if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
            path.parent()
                .map(|parent| canonicalize_path(&parent.to_string_lossy()))
                .unwrap_or_else(|| repo_root_canonical.clone())
        } else {
            repo_root_canonical.clone()
        }''',
        "checked git-parent unwrap",
    )
    path.write_text(text)


def repair_tools() -> None:
    path = Path("src/tools.rs")
    text = path.read_text()
    text = replace_once(
        text,
        '''        interactive_prompt_arg: if supports_interactive_prompt {
            Some("-i".to_string())
        } else {
            Some("-i".to_string())
        },''',
        '''        interactive_prompt_arg: if supports_interactive_prompt {
            Some("-i".to_string())
        } else {
            None
        },''',
        "truthful interactive capability result",
    )

    start = text.index("    let mut stdout_log: Option<String> = None;")
    end = text.index(
        '\n    let stream = args.stream.as_deref().unwrap_or("both");', start
    )
    replacement = '''    let (stdout_log, stderr_log, status) = if let Some(ref sid) = args.session_id {
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
    };'''
    text = text[:start] + replacement + text[end:]

    text = replace_once(
        text,
        '''    if matching_run.is_some() {
        let ns_str = summary.normalized_status.as_str().to_string();
        store
            .update(
                &matching_run.unwrap().id,
                AgentRunPatch {
                    normalized_status: Some(ns_str),
                    last_activity_time: summary.latest_activity_time.clone(),
                    last_reconciled_at: Some(Utc::now().to_rfc3339()),
                    remote_state: summary.state.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
    }''',
        '''    if let Some(run) = matching_run {
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
    }''',
        "Jules matching-run unwrap",
    )

    unsupported = {
        "CopilotRunInput": {
            "model",
            "agent",
            "prompt_file",
            "available_tools",
            "excluded_tools",
            "output_mode",
        },
        "CopilotFleetInput": {
            "model",
            "agent",
            "allow_all_tools",
            "available_tools",
            "excluded_tools",
        },
        "CopilotDelegateInput": {"model", "agent", "allow_all_tools"},
        "CopilotAutopilotInput": {"model", "agent", "allow_all_tools"},
        "CopilotKeepAliveInput": {"model", "agent", "allow_all_tools"},
        "CopilotReviewInput": {"model", "agent", "output_mode"},
    }
    for struct_name, fields in unsupported.items():
        text = remove_struct_fields(text, struct_name, fields)
    path.write_text(text)


def repair_registration() -> None:
    path = Path("src/registration.rs")
    text = path.read_text()
    sections = {
        "// copilot.run": {"model", "agent", "promptFile", "outputMode"},
        "// copilot.fleet": {"model", "agent", "allowAllTools"},
        "// copilot.delegate": {"model", "agent", "allowAllTools"},
        "// copilot.autopilot": {"model", "agent", "allowAllTools"},
        "// copilot.keep_alive": {"model", "agent", "allowAllTools"},
        "// copilot.review": {"model", "agent", "outputMode"},
    }
    for marker, fields in sections.items():
        text = remove_schema_properties(text, marker, fields)
    path.write_text(text)


def main() -> None:
    repair_config()
    repair_policy()
    repair_tools()
    repair_registration()


if __name__ == "__main__":
    main()
