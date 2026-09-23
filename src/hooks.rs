//! Normalize supported hook envelopes; never retain prompts, output, or transcripts.
use crate::model::{Event, PendingRequest, Provider, Status, ToolIdentity, WaitUpdate, clean};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn string<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| v.get(key).and_then(Value::as_str))
}

fn tool_identity(v: &Value, name: &str) -> ToolIdentity {
    let call_id = string(
        v,
        &[
            "tool_use_id",
            "toolUseId",
            "tool_call_id",
            "toolCallId",
            "call_id",
            "callId",
        ],
    )
    .filter(|s| !s.is_empty())
    .map(|s| clean(s, 180));
    let input = v.get("tool_input").or_else(|| v.get("toolInput"));
    // Codex PermissionRequest omits tool_use_id and adds a description to Bash's
    // input. Match the command itself with Pre/PostToolUse, without retaining it.
    let fingerprint = input.and_then(|input| {
        let input = if matches!(name, "Bash" | "apply_patch") {
            input.get("command")?
        } else {
            input
        };
        let mut hash = Sha256::new();
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(serde_json::to_vec(input).ok()?);
        Some(format!("{:x}", hash.finalize()))
    });
    ToolIdentity {
        call_id,
        fingerprint,
    }
}

fn question_tool(name: &str) -> Option<bool> {
    let leaf = name.rsplit(['.', ':']).next()?.rsplit("__").next()?;
    match leaf {
        "request_user_input_async" => Some(true),
        "AskUserQuestion" | "request_user_input" | "ask_user" | "ask_user_question" => Some(false),
        _ => None,
    }
}

pub fn normalize(provider: Provider, v: &Value, now: u64) -> Option<Event> {
    // Child hooks often carry the parent's session id. They must not finish it.
    if string(v, &["agent_id", "agentId", "subagentType"]).is_some() {
        return None;
    }
    let name = string(v, &["hook_event_name", "hookEventName", "type"])?;
    let name: String = name
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    let id = clean(string(v, &["session_id", "sessionId", "thread-id"])?, 180);
    if id.is_empty() {
        return None;
    }
    let project = string(v, &["cwd", "workspaceRoot"])
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|p| p.to_str())
        .map(|s| clean(s, 64))
        .unwrap_or_default();
    let raw_tool = string(v, &["tool_name", "toolName"]).unwrap_or("Tool");
    let tool = clean(raw_tool, 60);
    let (status, detail) = match name.as_str() {
        "sessionstart" => (Status::Idle, "Connected".into()),
        "userpromptsubmit" => (Status::Working, "Thinking".into()),
        "pretooluse" => {
            if question_tool(raw_tool).is_some() {
                (Status::Waiting, "Question to answer".into())
            } else {
                (Status::Working, tool)
            }
        }
        "posttooluse" | "posttoolusefailure" => (Status::Working, "Thinking".into()),
        // Codex emits this before either automatic review or a human decision.
        // Its hook envelope does not identify the reviewer or confirm a prompt.
        "permissionrequest" if provider == Provider::Codex => {
            (Status::Working, "Checking permissions".into())
        }
        "permissionrequest" => (Status::Waiting, "Permission requested".into()),
        // A denial has already been decided; it is never an outstanding prompt.
        "permissiondenied" => (Status::Working, "Tool permission denied".into()),
        "precompact" => (Status::Working, "Compacting context".into()),
        "postcompact" => (Status::Working, "Thinking".into()),
        "stop" | "agentturncomplete" => {
            if provider == Provider::Grok
                && matches!(string(v, &["reason"]), Some("channel_closed" | "shutdown"))
            {
                (Status::Ended, "Session closed".into())
            } else {
                let has_background = ["backgroundTasks", "background_tasks"].iter().any(|k| {
                    v.get(k)
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty())
                });
                // The foreground turn has ended. A scheduled wakeup is not work now.
                if has_background {
                    (Status::Ready, "Turn ended · background tasks remain".into())
                } else {
                    (Status::Ready, "Turn ended".into())
                }
            }
        }
        "stopfailure" => (Status::Error, "Agent reported a turn error".into()),
        "stopcancelled" | "interrupt" => (Status::Interrupted, "Turn interrupted".into()),
        "sessionend" => (Status::Ended, "Session closed".into()),
        "notification" => match string(v, &["notification_type", "notificationType"]) {
            Some("permission_prompt" | "elicitation_dialog") => {
                (Status::Waiting, "Permission requested".into())
            }
            Some("idle_prompt") => (Status::Idle, "Waiting for a prompt".into()),
            // Background task completion does not imply that the main turn ended.
            _ => return None,
        },
        _ => return None,
    };
    // Codex exposes PermissionRequest and tool input suitable for correlation.
    // Claude/Grok also use uncorrelated Notification prompts; preserve their
    // existing lifecycle behavior rather than leaving those waits stuck forever.
    let wait_update = if provider != Provider::Codex
        || matches!(
            name.as_str(),
            "sessionstart"
                | "userpromptsubmit"
                | "sessionend"
                | "stop"
                | "agentturncomplete"
                | "stopfailure"
                | "stopcancelled"
                | "interrupt"
        )
        || (name == "notification" && status == Status::Idle)
    {
        WaitUpdate::Clear
    } else if status == Status::Waiting || name == "permissionrequest" {
        WaitUpdate::Request(PendingRequest {
            tool: tool_identity(v, raw_tool),
            detail: detail.clone(),
            requested_ms: now,
            question: name == "pretooluse" && question_tool(raw_tool).is_some(),
            awaiting_user_reply: name == "pretooluse" && question_tool(raw_tool) == Some(true),
        })
    } else if matches!(
        name.as_str(),
        "posttooluse" | "posttoolusefailure" | "permissiondenied"
    ) {
        WaitUpdate::ToolFinished(tool_identity(v, raw_tool))
    } else {
        WaitUpdate::Preserve
    };
    Some(Event {
        provider,
        id,
        project,
        status,
        detail,
        turn_id: string(v, &["turn_id", "turn-id", "promptId"]).map(|s| clean(s, 180)),
        starts_turn: name == "userpromptsubmit",
        at_ms: now,
        origin: None,
        terminal: None,
        wait_update,
    })
}

pub fn events(provider: Provider) -> Vec<&'static str> {
    let mut names = vec![
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PreCompact",
        "PostCompact",
        "Stop",
    ];
    match provider {
        Provider::Codex => names.extend(["PermissionRequest", "Interrupt"]),
        Provider::Claude => names.extend([
            "PermissionRequest",
            "Notification",
            "StopFailure",
            "PostToolUseFailure",
        ]),
        Provider::Grok => names.extend([
            "Notification",
            "StopFailure",
            "StopCancelled",
            "PostToolUseFailure",
            "PermissionDenied",
        ]),
    }
    names
}
