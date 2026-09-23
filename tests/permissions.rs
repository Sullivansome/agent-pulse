use island_plugin_agent_pulse::{
    hooks,
    model::{self, Provider, Sessions, Status},
    store,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn event(name: &str, call: Option<&str>, command: &str) -> Value {
    let mut value = json!({
        "session_id": "codex-permission-test", "turn_id": "turn-1",
        "hook_event_name": name, "tool_name": "Bash",
        "tool_input": {"command": command}, "cwd": "/private/work/example"
    });
    if let Some(call) = call {
        value["tool_use_id"] = call.into();
    }
    if name == "PermissionRequest" {
        // Codex adds this only to the approval envelope, which has no call ID.
        value["tool_input"]["description"] = "PRIVATE_APPROVAL_REASON".into();
    }
    value
}

fn apply(state: &mut Sessions, value: Value, at: u64) {
    state.apply(hooks::normalize(Provider::Codex, &value, at).unwrap());
}

fn send(dir: &Path, value: Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
        .args(["hook", "--provider", "codex", "--data-dir"])
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(value.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({})
    );
}

#[test]
fn real_receivers_correlate_reviews_without_interrupting_parallel_work() {
    let dir = tempfile::tempdir().unwrap();
    send(dir.path(), event("UserPromptSubmit", None, ""));
    send(
        dir.path(),
        event("PermissionRequest", None, "PRIVATE_PENDING_COMMAND"),
    );
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = dir.path().to_path_buf();
            std::thread::spawn(move || {
                for name in ["PreToolUse", "PostToolUse"] {
                    send(
                        &path,
                        event(name, Some(&format!("parallel-{i}")), "unrelated command"),
                    );
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let state = store::load(dir.path()).unwrap();
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(!state.sessions[0].needs_attention());
    assert_eq!(state.sessions[0].activity_label(), "Working");
    assert_eq!(state.sessions[0].pending_requests.len(), 1);
    let now = island_plugin_agent_pulse::now_ms();
    assert_eq!(model::presentation(&state.visible(now), now).unwrap().0, 25);
    let stored = std::fs::read_to_string(dir.path().join("sessions.json")).unwrap();
    assert!(!stored.contains("PRIVATE_"));
    assert!(!stored.contains("/private/work"));
    // A fresh hook process reloads the wait and correlates despite description/ID differences.
    send(
        dir.path(),
        event(
            "PostToolUse",
            Some("approved-call"),
            "PRIVATE_PENDING_COMMAND",
        ),
    );
    let state = store::load(dir.path()).unwrap();
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(state.sessions[0].pending_requests.is_empty());
}

#[test]
fn each_request_resolves_independently_and_explicit_call_ids_take_precedence() {
    let mut state = Sessions::default();
    apply(
        &mut state,
        event("PermissionRequest", Some("a"), "same command"),
        1,
    );
    apply(
        &mut state,
        event("PermissionRequest", Some("b"), "same command"),
        2,
    );
    apply(
        &mut state,
        event("PostToolUse", Some("unrelated"), "same command"),
        3,
    );
    assert_eq!(state.sessions[0].pending_requests.len(), 2);
    apply(
        &mut state,
        event("PostToolUse", Some("a"), "same command"),
        4,
    );
    assert_eq!(state.sessions[0].pending_requests.len(), 1);
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(!state.sessions[0].needs_attention());
    apply(
        &mut state,
        event("PostToolUse", Some("b"), "same command"),
        5,
    );
    assert_eq!(state.sessions[0].status, Status::Working);

    // Two approvals without IDs for the same command must not collapse into one.
    for at in [6, 7] {
        apply(
            &mut state,
            event("PermissionRequest", None, "same command"),
            at,
        );
    }
    apply(
        &mut state,
        event("PostToolUse", Some("c"), "same command"),
        8,
    );
    assert_eq!(state.sessions[0].pending_requests.len(), 1);
    apply(
        &mut state,
        event("PostToolUse", Some("d"), "same command"),
        9,
    );
    assert_eq!(state.sessions[0].status, Status::Working);
}

#[test]
fn parallel_hook_arrival_order_does_not_lose_waits_or_resurrect_ended_turns() {
    let mut state = Sessions::default();
    apply(&mut state, event("UserPromptSubmit", None, ""), 1);
    apply(
        &mut state,
        event("PostToolUse", Some("parallel"), "unrelated"),
        4,
    );
    apply(&mut state, event("PermissionRequest", None, "pending"), 3);
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(!state.sessions[0].needs_attention());
    assert_eq!(state.sessions[0].activity_label(), "Working");
    assert_eq!(state.sessions[0].updated_ms, 4);
    apply(
        &mut state,
        event("PreToolUse", Some("another"), "unrelated"),
        6,
    );
    apply(
        &mut state,
        event("PostToolUse", Some("approved"), "pending"),
        5,
    );
    assert_eq!(state.sessions[0].status, Status::Working);
    apply(&mut state, event("Stop", None, ""), 8);
    apply(&mut state, event("PermissionRequest", None, "late"), 7);
    assert_eq!(state.sessions[0].status, Status::Ready);
}

#[test]
fn turn_boundaries_clear_waits_and_late_old_turn_events_are_ignored() {
    for name in ["Stop", "Interrupt", "SessionEnd", "UserPromptSubmit"] {
        let mut state = Sessions::default();
        apply(&mut state, event("PermissionRequest", None, "pending"), 1);
        apply(&mut state, event(name, None, ""), 2);
        assert!(state.sessions[0].pending_requests.is_empty(), "{name}");
        assert_ne!(state.sessions[0].status, Status::Waiting, "{name}");
    }
    let mut state = Sessions::default();
    let mut start = event("UserPromptSubmit", None, "");
    start["turn_id"] = "turn-2".into();
    apply(&mut state, start, 2);
    apply(&mut state, event("PermissionRequest", None, "old turn"), 3);
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(state.sessions[0].pending_requests.is_empty());
}

#[test]
fn codex_turn_end_wins_over_reordered_and_late_waiting_signals() {
    for end in ["Stop", "Interrupt", "SessionEnd"] {
        let mut state = Sessions::default();
        apply(&mut state, event("UserPromptSubmit", None, ""), 1);
        apply(&mut state, event("PermissionRequest", None, "pending"), 5);
        // Completion's receiver started first but reached the lock last.
        apply(&mut state, event(end, None, ""), 4);
        assert_ne!(state.sessions[0].status, Status::Waiting, "{end}");
        assert!(state.sessions[0].pending_requests.is_empty());
        let settled = state.sessions[0].status;
        for (i, tool) in ["PostToolUse", "PermissionRequest", "PreToolUse"]
            .iter()
            .enumerate()
        {
            apply(
                &mut state,
                event(tool, Some("late"), "pending"),
                6 + i as u64,
            );
            assert_eq!(state.sessions[0].status, settled, "late {tool} after {end}");
            assert!(state.sessions[0].pending_requests.is_empty());
        }
        let mut next = event("UserPromptSubmit", None, "");
        next["turn_id"] = "turn-2".into();
        apply(&mut state, next, 10);
        assert_eq!(state.sessions[0].status, Status::Working);
    }
}

#[test]
fn async_questions_remain_pending_after_the_tool_returns_and_legacy_waits_survive() {
    for name in [
        "functions.request_user_input_async",
        "mcp__tools__request_user_input_async",
    ] {
        let mut state = Sessions::default();
        let mut question = event("PreToolUse", Some("question"), "");
        question["tool_name"] = name.into();
        apply(&mut state, question.clone(), 1);
        question["hook_event_name"] = "PostToolUse".into();
        apply(&mut state, question, 2);
        assert_eq!(state.sessions[0].status, Status::Waiting);
        apply(&mut state, event("UserPromptSubmit", None, ""), 3);
        assert_eq!(state.sessions[0].status, Status::Working);
    }
    let mut state = Sessions::default();
    let mut question = event("PreToolUse", Some("question"), "");
    question["tool_name"] = "functions.request_user_input".into();
    apply(&mut state, question.clone(), 1);
    question["hook_event_name"] = "PostToolUse".into();
    apply(&mut state, question, 2);
    assert_eq!(state.sessions[0].status, Status::Working);

    apply(&mut state, event("PermissionRequest", None, "pending"), 3);
    let mut legacy = serde_json::to_value(&state).unwrap();
    legacy["sessions"][0]
        .as_object_mut()
        .unwrap()
        .remove("pending_requests");
    legacy["sessions"][0]["status"] = "waiting".into();
    legacy["sessions"][0]["detail"] = "Permission requested".into();
    let mut state: Sessions = serde_json::from_value(legacy).unwrap();
    apply(
        &mut state,
        event("PostToolUse", Some("unrelated"), "unrelated"),
        4,
    );
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(!state.sessions[0].needs_attention());
    assert_eq!(state.sessions[0].activity_label(), "Working");
    apply(&mut state, event("Stop", None, ""), 5);
    assert_eq!(state.sessions[0].status, Status::Ready);
}

#[test]
fn review_overflow_is_bounded_and_never_invents_a_human_question() {
    let mut state = Sessions::default();
    for i in 0..50 {
        apply(
            &mut state,
            event("PermissionRequest", Some(&format!("call-{i}")), "pending"),
            i,
        );
    }
    assert_eq!(state.sessions[0].pending_requests.len(), 16);
    for i in 0..50 {
        apply(
            &mut state,
            event("PostToolUse", Some(&format!("call-{i}")), "pending"),
            i + 50,
        );
    }
    assert_eq!(state.sessions[0].status, Status::Working);
    assert!(!state.sessions[0].needs_attention());
    apply(&mut state, event("Stop", None, ""), 100);
    assert!(state.sessions[0].pending_requests.is_empty());
}

#[test]
fn missing_build_completion_does_not_mask_fresh_work_or_discard_an_approval() {
    use island_plugin_agent_pulse::render;
    let mut state = Sessions::default();
    apply(&mut state, event("UserPromptSubmit", None, ""), 1);
    apply(
        &mut state,
        event("PreToolUse", Some("build"), "build fixture"),
        2,
    );
    apply(
        &mut state,
        event("PermissionRequest", None, "build fixture"),
        3,
    );
    assert_eq!(render::summary(&state.sessions), "Checking permissions");
    assert!(!render::summary(&state.sessions).contains("needs you"));
    // Replays the observed desktop path: the build yields, then unrelated work
    // runs, but no PostToolUse arrives for the unpolled build result.
    apply(
        &mut state,
        event("PreToolUse", Some("inspect"), "inspect fixture"),
        4,
    );
    apply(
        &mut state,
        event("PostToolUse", Some("inspect"), "inspect fixture"),
        5,
    );
    assert_eq!(state.sessions[0].status, Status::Working);
    assert_eq!(state.sessions[0].pending_requests.len(), 1);
    assert!(!state.sessions[0].needs_attention());
    assert_eq!(state.sessions[0].activity_label(), "Working");
    assert_eq!(render::summary(&state.sessions), "1 working");
    let before_completion = model::presentation(&state.sessions, 5).unwrap();
    assert_eq!(before_completion.0, 25);
    // A known human prompt outranks a session with an unmatched review.
    state.sessions.push(render::samples(100_000).remove(0));
    assert_eq!(state.visible(100_000)[0].provider, Provider::Claude);
    state.sessions.pop();
    // A real answer or turn boundary remains necessary to remove the request.
    apply(
        &mut state,
        event("PostToolUse", Some("build"), "build fixture"),
        6,
    );
    assert!(!state.sessions[0].needs_attention());
    let working = model::presentation(&state.sessions, 6).unwrap();
    assert_eq!(working.0, 25);
    assert_eq!(
        before_completion.2, working.2,
        "review bookkeeping must not re-present the island"
    );
}

#[test]
fn questions_still_need_the_user_while_unrelated_work_continues() {
    use island_plugin_agent_pulse::render;
    for tool in ["request_user_input", "request_user_input_async"] {
        let mut state = Sessions::default();
        let mut question = event("PreToolUse", Some("question"), "");
        question["tool_name"] = tool.into();
        apply(&mut state, question, 1);
        apply(
            &mut state,
            event("PostToolUse", Some("other"), "unrelated"),
            2,
        );
        assert_eq!(state.sessions[0].status, Status::Waiting);
        assert!(state.sessions[0].needs_attention());
        assert_eq!(render::summary(&state.sessions), "1 needs you");
        assert_eq!(model::presentation(&state.sessions, 2).unwrap().0, 90);
    }
}

#[test]
fn permission_modes_do_not_identify_a_human_reviewer() {
    use island_plugin_agent_pulse::render;
    for mode in [
        "default",
        "acceptEdits",
        "plan",
        "dontAsk",
        "bypassPermissions",
    ] {
        let mut state = Sessions::default();
        let mut review = event("PermissionRequest", None, "fixture");
        review["permission_mode"] = mode.into();
        apply(&mut state, review, 1);
        assert_eq!(state.sessions[0].activity_label(), "Checking permissions");
        assert!(!state.sessions[0].needs_attention());
        assert_eq!(render::summary(&state.sessions), "Checking permissions");
        assert_eq!(model::presentation(&state.sessions, 1).unwrap().0, 25);

        // Missing completion eventually becomes unknown, never a human warning.
        let visible = state.visible(model::STALE_MS + 2);
        assert_eq!(visible[0].status, Status::Unknown);
        assert!(!visible[0].needs_attention());
        assert!(model::presentation(&visible, model::STALE_MS + 2).is_none());
    }
}

#[test]
fn stored_approval_warnings_are_reconciled_without_waiting_for_another_hook() {
    let mut state = Sessions::default();
    apply(&mut state, event("PermissionRequest", None, "fixture"), 1);
    state.sessions[0].status = Status::Waiting;
    state.sessions[0].detail = "Permission requested".into();
    let visible = state.visible(2);
    assert_eq!(visible[0].activity_label(), "Checking permissions");
    assert!(!visible[0].needs_attention());

    state.sessions[0].status = Status::Working;
    state.sessions[0].detail = "Approval unconfirmed".into();
    state.sessions[0].last_working_ms = 2;
    let visible = state.visible(3);
    assert_eq!(visible[0].activity_label(), "Working");
    assert!(!visible[0].needs_attention());

    // Older stores did not have pending request metadata at all.
    state.sessions[0].status = Status::Waiting;
    state.sessions[0].detail = "Permission requested".into();
    state.sessions[0].pending_requests.clear();
    assert_eq!(state.visible(3)[0].activity_label(), "Checking permissions");

    state.sessions[0].detail = "Question to answer".into();
    assert!(state.visible(3)[0].needs_attention());
}

#[test]
fn a_real_question_survives_permission_record_overflow() {
    let mut state = Sessions::default();
    for at in 1..20 {
        apply(&mut state, event("PermissionRequest", None, "fixture"), at);
    }
    let mut question = event("PreToolUse", Some("question"), "");
    question["tool_name"] = "request_user_input_async".into();
    apply(&mut state, question, 20);
    for at in 21..40 {
        apply(&mut state, event("PermissionRequest", None, "fixture"), at);
    }
    assert!(state.sessions[0].needs_attention());
    assert_eq!(state.sessions[0].pending_requests.len(), 16);
    apply(&mut state, event("UserPromptSubmit", None, ""), 40);
    assert!(!state.sessions[0].needs_attention());
}
