#[cfg(unix)]
use island_plugin_agent_pulse::setup;
use island_plugin_agent_pulse::{
    hooks,
    model::{self, Provider, Sessions, Status},
    render::{self, Action, Renderer, Ui},
};
use serde_json::json;
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn send(dir: &std::path::Path, provider: &str, value: serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
        .args(["hook", "--provider", provider, "--data-dir"])
        .arg(dir)
        .env_remove("GROK_SESSION_ID")
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
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap(),
        json!({})
    );
}

#[test]
fn real_hook_processes_share_private_state_without_collecting_content() {
    let temp = tempfile::tempdir().unwrap();
    for provider in Provider::ALL {
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
        ] {
            send(
                temp.path(),
                provider.key(),
                json!({"session_id": provider.key(), "hook_event_name": event,
                "cwd": "/private/work/island", "prompt": "SECRET_PROMPT", "tool_response": "SECRET_OUTPUT", "transcript_path": "SECRET_PATH"}),
            );
        }
    }
    let state = island_plugin_agent_pulse::store::load(temp.path()).unwrap();
    assert_eq!(state.sessions.len(), 3);
    assert!(state.sessions.iter().all(|s| s.status == Status::Ready));
    let text = std::fs::read_to_string(temp.path().join("sessions.json")).unwrap();
    assert!(!text.contains("SECRET"));
    assert!(!text.contains("/private/work"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(temp.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(temp.path().join("sessions.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn grok_camelcase_background_tasks_and_old_turns_are_handled() {
    let mut state = Sessions::default();
    for (name, turn, at) in [
        ("user_prompt_submit", "a", 1),
        ("user_prompt_submit", "b", 2),
        ("stop", "a", 3),
    ] {
        let event = hooks::normalize(Provider::Grok, &json!({"sessionId":"g1", "hookEventName":name, "promptId":turn, "workspaceRoot":"/tmp/work"}), at).unwrap();
        state.apply(event);
    }
    assert_eq!(state.sessions[0].status, Status::Working);
    let waiting = hooks::normalize(Provider::Grok, &json!({"sessionId":"g1", "hookEventName":"notification", "notificationType":"permission_prompt"}), 4).unwrap();
    state.apply(waiting);
    assert_eq!(state.sessions[0].status, Status::Waiting);
    let bg = hooks::normalize(Provider::Grok, &json!({"sessionId":"g1", "hookEventName":"stop", "promptId":"b", "backgroundTasks":[{"id":"bg"}]}), 5).unwrap();
    assert_eq!(bg.status, Status::Ready);
    state.apply(bg);
    assert_eq!(state.visible(model::STALE_MS + 10)[0].status, Status::Ready);
    assert!(state.visible(model::RETAIN_MS + 10).is_empty());
}

#[test]
fn child_signals_unknown_notifications_and_terminal_escapes_cannot_corrupt_status() {
    for provider in Provider::ALL {
        assert!(
            hooks::normalize(
                provider,
                &json!({"session_id":"parent", "agent_id":"child", "hook_event_name":"Stop"}),
                1
            )
            .is_none()
        );
        assert!(hooks::normalize(provider, &json!({"session_id":"parent", "hook_event_name":"Notification", "notification_type":"task_complete"}), 1).is_none());
    }
    assert!(
        hooks::normalize(
            Provider::Grok,
            &json!({"sessionId":"p", "subagentType":"explore", "hookEventName":"session_end"}),
            1
        )
        .is_none()
    );
    let e = hooks::normalize(Provider::Claude, &json!({"session_id":"../../evil\u{001b}", "hook_event_name":"UserPromptSubmit", "cwd":"/a/project\nname"}), 1).unwrap();
    assert_eq!(e.project, "projectname");
    assert!(!e.id.contains('\u{001b}'));
}

#[test]
fn tool_updates_do_not_represent_or_reset_user_collapse() {
    let mut state = Sessions::default();
    let mut initial_signature = None;
    for (index, event) in ["UserPromptSubmit", "PreToolUse", "PostToolUse"]
        .iter()
        .enumerate()
    {
        state.apply(
            hooks::normalize(
                Provider::Codex,
                &json!({"session_id":"c", "hook_event_name":event, "tool_name":"Bash"}),
                100 + index as u64,
            )
            .unwrap(),
        );
        let signature = model::presentation(&state.visible(105), 105).unwrap().2;
        assert_eq!(
            initial_signature.get_or_insert_with(|| signature.clone()),
            &signature
        );
    }
    state.apply(
        hooks::normalize(
            Provider::Codex,
            &json!({"session_id":"c", "hook_event_name":"PermissionRequest"}),
            106,
        )
        .unwrap(),
    );
    assert_eq!(model::presentation(&state.visible(107), 107).unwrap().0, 25);
}

#[test]
#[cfg(unix)]
fn setup_is_idempotent_preserves_other_hooks_and_removes_only_ours() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data with 'quote'");
    std::fs::create_dir_all(temp.path().join(".claude")).unwrap();
    let path = temp.path().join(".claude/settings.json");
    let original = json!({"permissions":{"allow":["Read"]}, "hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo unrelated"}]}]}});
    std::fs::write(&path, original.to_string()).unwrap();
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"));
    let install =
        || setup::configure(temp.path(), &data, binary, &Provider::ALL, false, false).unwrap();
    assert_eq!(install().len(), 3);
    assert!(install().is_empty());
    let installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(installed["permissions"], original["permissions"]);
    let command = installed["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let mut child = Command::new("sh")
        .args(["-c", command])
        .env_remove("GROK_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"quoted-path","hook_event_name":"UserPromptSubmit"}"#)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        island_plugin_agent_pulse::store::load(&data)
            .unwrap()
            .sessions
            .len(),
        1
    );
    setup::configure(temp.path(), &data, binary, &Provider::ALL, true, false).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap(),
        original
    );
}

#[test]
#[cfg(unix)]
fn broken_config_aborts_before_modifying_other_configs() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".codex")).unwrap();
    std::fs::write(temp.path().join(".codex/hooks.json"), b"not json").unwrap();
    assert!(
        setup::configure(
            temp.path(),
            &temp.path().join("data"),
            std::path::Path::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse")),
            &Provider::ALL,
            false,
            false
        )
        .is_err()
    );
    assert!(!temp.path().join(".claude/settings.json").exists());
}

#[test]
fn concurrent_sessions_do_not_lose_updates() {
    let temp = tempfile::tempdir().unwrap();
    let threads: Vec<_> = (0..20)
        .map(|i| {
            let path = temp.path().to_path_buf();
            std::thread::spawn(move || {
                send(
                    &path,
                    "codex",
                    json!({"session_id":format!("c-{i}"),"hook_event_name":"UserPromptSubmit"}),
                )
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(
        island_plugin_agent_pulse::store::load(temp.path())
            .unwrap()
            .sessions
            .len(),
        20
    );
}

#[test]
fn render_and_input_remain_bounded_across_host_sizes() {
    let renderer = Renderer::new();
    for (w, h, scale) in [
        (245, 24, 2.0),
        (380, 208, 2.0),
        (512, 256, 3.0),
        (0, 0, f32::NAN),
    ] {
        let frame = renderer.frame(
            w,
            h,
            scale,
            &render::samples(100_000),
            &Ui::default(),
            100_000,
        );
        frame.validate().unwrap();
        assert_eq!(
            frame.pixels.len(),
            (frame.width * frame.height * 4) as usize
        );
    }
    let samples = render::samples(100_000);
    let ui = render::Ui::default();
    assert_eq!(
        render::hit(340, 192, &samples, &ui, 35.0, 60.0),
        Some(Action::Row(0))
    );
    assert_eq!(
        render::hit(340, 192, &samples, &ui, 320.0, 13.0),
        Some(Action::Menu)
    );
    assert_eq!(render::hit(340, 192, &samples, &ui, f32::NAN, 13.0), None);
}

#[test]
fn activity_layout_and_secondary_controls_follow_content() {
    use render::{Action, Page, Ui};
    let samples = render::samples(100_000);
    assert_eq!(render::preferred_height(1, Page::Sessions), 88);
    assert_eq!(render::preferred_height(2, Page::Sessions), 140);
    assert_eq!(render::preferred_height(128, Page::Sessions), 192);
    let ui = Ui::default();
    assert_eq!(render::actions(&samples, &ui), vec![Action::Menu]);
    let menu = Ui {
        page: Page::Menu,
        ..Ui::default()
    };
    assert_eq!(
        render::actions(&samples, &menu),
        vec![Action::Back, Action::Copy, Action::Settings]
    );
    assert_eq!(
        render::hit(340, 176, &samples, &menu, 30.0, 90.0),
        Some(Action::Settings)
    );
    let frame = render::Renderer::new().frame(340, 88, 2.0, &samples[..1], &ui, 100_000);
    // A parser accepting an empty system-font face must not produce a blank card.
    let bright = (60..96)
        .flat_map(|y| (108..430).map(move |x| (y * 680 + x) * 4))
        .filter(|i| frame.pixels[*i] > 160 && frame.pixels[*i + 1] > 160)
        .count();
    assert!(bright > 40, "Project title must contain visible glyphs");
}

#[test]
fn a_completed_turn_gets_attention_while_other_agents_keep_working() {
    let now = 100_000;
    let mut sessions = render::samples(now);
    sessions.retain(|s| s.status != Status::Waiting);
    let (priority, sticky, signature) = model::presentation(&sessions, now).unwrap();
    assert_eq!(priority, 55);
    assert!(!sticky);
    assert!(signature.contains("Ready"));
    assert_eq!(model::presentation(&sessions, now + 13_000).unwrap().0, 25);
}
#[test]
fn shipped_manifest_loads_with_required_capabilities() {
    use island_plugin_api::{Permission, PluginManifest};
    let manifest = PluginManifest::parse(include_str!("../island-plugin.toml")).unwrap();
    manifest.validate().unwrap();
    let grants = manifest.permission_set().unwrap();
    assert!(grants.allows(&Permission::ShellOpenUrl));
    assert!(grants.allows(&Permission::Present));
    assert!(grants.allows(&Permission::SurfaceFramebuffer));
}

#[test]
fn human_requests_remain_above_work_even_after_a_long_wait() {
    let now = 10_000_000;
    let mut sessions = render::samples(now);
    sessions[0].updated_ms = now - model::STALE_MS - 1;
    let state = Sessions { sessions };
    let visible = state.visible(now);
    assert_eq!(visible[0].status, Status::Waiting);
    let (priority, sticky, _) = model::presentation(&visible, now).unwrap();
    assert_eq!(priority, 90);
    assert!(sticky);
    assert_eq!(
        model::presentation_mode(priority),
        island_plugin_api::PresentationMode::Hover
    );
    assert_eq!(
        model::presentation_mode(25),
        island_plugin_api::PresentationMode::Hover
    );
    assert_eq!(
        model::presentation_mode(55),
        island_plugin_api::PresentationMode::Hover
    );
    assert!(
        !state
            .visible(now + model::RETAIN_MS)
            .iter()
            .any(|s| s.status == Status::Waiting)
    );
}

#[test]
fn grok_denials_settle_the_request_and_only_turn_end_events_settle_the_turn() {
    for (event, extra, expected) in [
        ("permission_denied", json!({}), Status::Working),
        (
            "stop_cancelled",
            json!({"reason":"permission_rejected"}),
            Status::Interrupted,
        ),
        (
            "stop",
            json!({"sessionCrons":[{"id":"later"}]}),
            Status::Ready,
        ),
        ("stop", json!({"reason":"shutdown"}), Status::Ended),
        (
            "notification",
            json!({"notificationType":"idle_prompt"}),
            Status::Idle,
        ),
    ] {
        let mut payload = json!({"sessionId":"grok", "hookEventName": event});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            hooks::normalize(Provider::Grok, &payload, 100)
                .unwrap()
                .status,
            expected
        );
    }
}

#[test]
fn grok_registry_clears_abandoned_requests_but_preserves_fresh_and_live_hooks() {
    use island_plugin_agent_pulse::grok::{ActiveSession, reconcile};
    let now = 1_000_000;
    let mut sessions = render::samples(now);
    sessions[0].provider = Provider::Grok;
    sessions[0].updated_ms = now - 90_000;
    let original = sessions.clone();
    reconcile(
        &mut sessions,
        &[ActiveSession {
            session_id: sessions_id(&original),
            pid: 123,
        }],
        now,
    );
    assert_eq!(sessions[0].status, Status::Waiting);
    reconcile(&mut sessions, &[], now - 80_000);
    assert_eq!(sessions[0].status, Status::Waiting, "registry startup race");
    reconcile(&mut sessions, &[], now);
    assert_eq!(sessions[0].status, Status::Ended);
    assert_eq!(
        sessions[1].status, original[1].status,
        "other providers unaffected"
    );
}

#[test]
fn session_end_hides_rows_but_turn_end_keeps_an_open_session_available() {
    for provider in Provider::ALL {
        let mut state = Sessions::default();
        for (at, name, count) in [
            (1, "UserPromptSubmit", 1),
            (2, "Stop", 1),
            (3, "SessionEnd", 0),
            (4, "UserPromptSubmit", 1),
        ] {
            state.apply(
                hooks::normalize(
                    provider,
                    &json!({"session_id":"closed", "hook_event_name":name}),
                    at,
                )
                .unwrap(),
            );
            assert_eq!(state.visible(at).len(), count, "{provider:?}: {name}");
        }
    }
}

#[test]
fn a_new_turn_replaces_the_previous_terminal_owner() {
    let mut state = Sessions::default();
    let payload = json!({"session_id":"resumed", "hook_event_name":"UserPromptSubmit"});
    let mut event = hooks::normalize(Provider::Codex, &payload, 1).unwrap();
    event.terminal = Some(island_plugin_agent_pulse::process::TerminalProcess {
        pid: 123,
        started: "old".into(),
        terminal: 1,
    });
    state.apply(event);
    // Resuming the task in desktop must not inherit a closed terminal's lifetime.
    state.apply(hooks::normalize(Provider::Codex, &payload, 2).unwrap());
    assert!(state.sessions[0].terminal.is_none());
}
fn sessions_id(sessions: &[island_plugin_agent_pulse::model::Session]) -> String {
    sessions[0].id.clone()
}

#[test]
fn session_navigation_uses_desktop_thread_or_recorded_terminal_without_guessing() {
    use island_plugin_agent_pulse::navigation::{Origin, Target, target};
    let mut session = render::samples(100_000).remove(1);
    session.provider = Provider::Codex;
    session.id = "01234567-89ab-cdef-0123-456789abcdef".into();
    assert!(matches!(target(&session, true), Target::Url(_)));
    assert!(matches!(target(&session, false), Target::Unavailable));
    session.origin = Some(Origin {
        pid: 123,
        started_sec: 10,
        started_usec: 1,
        bundle_id: "com.mitchellh.ghostty".into(),
        name: "Ghostty".into(),
    });
    assert!(
        matches!(target(&session, true), Target::Application(_)),
        "CLI returns to its terminal"
    );
    session.provider = Provider::Grok;
    assert!(matches!(target(&session, true), Target::Application(_)));
    session.origin = None;
    assert!(
        matches!(target(&session, true), Target::Unavailable),
        "no unrelated terminal guessed"
    );
}

#[test]
fn duplicate_clicks_and_stale_replies_cannot_start_or_finish_another_navigation() {
    use island_plugin_agent_pulse::navigation::LaunchState;
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let mut launch = LaunchState::default();
    let first = launch.begin(now).unwrap();
    assert!(launch.begin(now + Duration::from_millis(100)).is_none());
    assert!(launch.finish(first));
    assert!(launch.begin(now + Duration::from_millis(200)).is_none());
    let second = launch.begin(now + Duration::from_secs(1)).unwrap();
    assert!(!launch.finish(first));
    assert!(!launch.timed_out(now + Duration::from_secs(4)));
    assert!(launch.timed_out(now + Duration::from_secs(6)));
    assert!(!launch.finish(second));
    assert!(launch.begin(now + Duration::from_secs(6)).is_some());
}
