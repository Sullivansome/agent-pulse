use island_plugin_agent_pulse::{
    connections::{self, Connections, Installation},
    model::{Provider, STALE_MS},
    render::{self, Action, Page, Renderer, Ui},
    setup,
};

#[test]
fn setup_marker_alone_is_not_connection_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(data.join("connected.json"), r#"["claude","codex","grok"]"#).unwrap();
    assert_eq!(
        connections::inspect(temp.path(), &data, false),
        [Installation::NotInstalled; 3]
    );
}

#[cfg(unix)]
#[test]
fn installation_checks_each_provider_destination_and_receiver_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    setup::configure(
        temp.path(),
        &data,
        &std::env::current_exe().unwrap(),
        &[Provider::Codex],
        false,
        false,
    )
    .unwrap();
    let path = setup::config_path(temp.path(), Provider::Codex, false);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        connections::inspect(temp.path(), &data, false),
        [
            Installation::NotInstalled,
            Installation::Installed,
            Installation::NotInstalled
        ]
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "inspection must never change trust or settings"
    );
    let mut config: serde_json::Value = serde_json::from_slice(&before).unwrap();
    config["hooks"].as_object_mut().unwrap().remove("Stop");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert_eq!(
        connections::inspect(temp.path(), &data, false)[1],
        Installation::Incomplete
    );
    std::fs::write(&path, before).unwrap();
    std::fs::remove_file(data.join("bin/island-agent-pulse")).unwrap();
    assert_eq!(
        connections::inspect(temp.path(), &data, false)[1],
        Installation::Incomplete
    );
    std::fs::write(&path, b"invalid json").unwrap();
    assert_eq!(
        connections::inspect(temp.path(), &data, false)[1],
        Installation::Unavailable
    );
    std::fs::write(&path, br#"{"hooks":{"Stop":"invalid groups"}}"#).unwrap();
    assert_eq!(
        connections::inspect(temp.path(), &data, false)[1],
        Installation::Unavailable
    );
}

#[test]
fn activity_is_per_provider_and_stale_history_never_claims_current_reception() {
    let now = 3_600_000;
    let mut state = Connections {
        installation: [Installation::Installed; 3],
        ..Connections::default()
    };
    assert!(!state.has_activity());
    assert_eq!(state.activity_label(1, now), "No recorded activity");
    assert_eq!(state.action_label(), "Reinstall hooks");
    let mut samples = render::samples(now);
    samples.retain(|s| s.provider == Provider::Codex);
    samples[0].updated_ms = now;
    state.update_activity(Some(&samples));
    assert!(state.recent(1, now));
    assert!(!state.recent(0, now));
    assert!(!state.recent(2, now));
    assert_eq!(state.activity_label(1, now), "Last signal just now");
    assert!(!state.recent(1, now + STALE_MS));
    assert_eq!(
        state.activity_label(1, now + STALE_MS),
        "Last signal 10m ago"
    );
    assert!(state.hint(now + STALE_MS)[0].contains("waiting for activity"));
    state.update_activity(None);
    assert!(!state.recent(1, now));
    assert_eq!(state.activity_label(1, now), "Can't read activity");
}

#[test]
fn connection_guidance_never_assumes_trust_or_repairs_a_quiet_installation() {
    let mut state = Connections {
        installation: [Installation::Installed; 3],
        ..Connections::default()
    };
    assert_eq!(state.action_label(), "Reinstall hooks");
    assert_eq!(
        state.hint(100)[1],
        "Start a new task to check the connection."
    );
    state.installation[0] = Installation::Incomplete;
    assert_eq!(state.action_label(), "Repair setup");
    state.installation[0] = Installation::NotInstalled;
    assert_eq!(state.action_label(), "Install hooks");
}

#[test]
fn connection_controls_and_provider_rows_fit_the_surface() {
    use island_design::agent_pulse as t;
    let mut ui = Ui {
        page: Page::Settings,
        ..Ui::default()
    };
    for scale in [1.0, 2.0, 3.0] {
        let frame = Renderer::new().frame(t::WIDTH, t::SETTINGS_HEIGHT, scale, &[], &ui, 100);
        frame.validate().unwrap();
    }
    assert_eq!(
        render::hit(
            t::WIDTH,
            t::SETTINGS_HEIGHT,
            &[],
            &ui,
            40.0,
            t::CONNECTION_ACTION_Y + 12.0
        ),
        Some(Action::Setup)
    );
    assert_eq!(
        render::hit(t::WIDTH, t::SETTINGS_HEIGHT, &[], &ui, 40.0, 150.0),
        None
    );
    ui.loading = true;
    assert_eq!(render::actions(&[], &ui), [Action::Back]);
    assert_eq!(
        render::hit(
            t::WIDTH,
            t::SETTINGS_HEIGHT,
            &[],
            &ui,
            40.0,
            t::CONNECTION_ACTION_Y + 12.0
        ),
        None
    );
}
