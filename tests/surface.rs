//! Real plugin process over the public host protocol; isolated hook store.
use island_plugin_agent_pulse::{hooks, model::Provider, navigation, now_ms, store};
use island_plugin_api::{
    FramePacket, HostToPlugin, InputEvent, PluginToHost, read_message, write_json,
};
use std::{
    io::{BufReader, Write},
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn authenticated_surface_reports_attention_accepts_input_and_shuts_down() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Process(
        Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
            .env("ISLAND_ENDPOINT", format!("tcp:{address}"))
            .env("ISLAND_PLUGIN_TOKEN", "fixture-token")
            .env("ISLAND_PLUGIN_ID", island_plugin_agent_pulse::PLUGIN_ID)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut socket, _) = loop {
        if let Ok(connection) = listener.accept() {
            break connection;
        }
        assert!(Instant::now() < deadline, "plugin did not connect");
        std::thread::sleep(Duration::from_millis(10));
    };
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    write_json(&mut socket, &HostToPlugin::hello("test", "fixture")).unwrap();
    let FramePacket::Json(hello) = read_message(&mut reader).unwrap() else {
        panic!("handshake expected")
    };
    assert!(
        matches!(serde_json::from_slice::<PluginToHost>(&hello).unwrap(), PluginToHost::HelloOk { token, .. } if token == "fixture-token")
    );
    write_json(
        &mut socket,
        &HostToPlugin::Configure {
            permissions: vec!["shell:open-url".into()],
            data_dir: temp.path().display().to_string(),
        },
    )
    .unwrap();
    write_json(
        &mut socket,
        &HostToPlugin::SurfaceReady {
            surface_id: 1,
            w: 340,
            h: 88,
            scale: 2.0,
        },
    )
    .unwrap();
    let mut hook = Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
        .args(["hook", "--provider", "grok", "--data-dir"])
        .arg(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    hook.stdin.take().unwrap().write_all(br#"{"sessionId":"real-surface","hookEventName":"notification","notificationType":"permission_prompt","cwd":"/tmp/surface"}"#).unwrap();
    assert!(hook.wait().unwrap().success());
    let mut attention = false;
    let mut attention_meta = false;
    let mut frame = false;
    while !attention || !attention_meta || !frame {
        assert!(Instant::now() < deadline);
        match read_message(&mut reader).unwrap() {
            FramePacket::Frame(pixels) => {
                pixels.validate().unwrap();
                assert_eq!(pixels.width, 680);
                frame = pixels.height == 176;
            }
            FramePacket::Json(bytes) => {
                let message = serde_json::from_slice::<PluginToHost>(&bytes).unwrap();
                if matches!(&message, PluginToHost::SetCollapsedMeta { dot: Some(dot), .. } if dot == "attention")
                {
                    attention_meta = true;
                }
                if let PluginToHost::PreferredSize { w, h } = message {
                    write_json(
                        &mut socket,
                        &HostToPlugin::SurfaceResize {
                            surface_id: 1,
                            w,
                            h,
                            scale: 2.0,
                        },
                    )
                    .unwrap();
                }
                if matches!(
                    message,
                    PluginToHost::Present {
                        priority: 90,
                        mode: island_plugin_api::PresentationMode::Hover,
                        sticky: true,
                        ..
                    }
                ) {
                    attention = true;
                }
            }
        }
    }
    for event in [
        InputEvent::PointerDown {
            x: 320.0,
            y: 13.0,
            button: 0,
        },
        InputEvent::PointerUp {
            x: 320.0,
            y: 13.0,
            button: 0,
        },
    ] {
        write_json(&mut socket, &HostToPlugin::Input { event }).unwrap();
    }
    loop {
        if let FramePacket::Json(bytes) = read_message(&mut reader).unwrap()
            && matches!(
                serde_json::from_slice::<PluginToHost>(&bytes).unwrap(),
                PluginToHost::PreferredSize { h: 176, .. }
            )
        {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    // Navigation must resize without a new Present or rerunning setup.
    write_json(
        &mut socket,
        &HostToPlugin::SurfaceResize {
            surface_id: 1,
            w: 340,
            h: 176,
            scale: 2.0,
        },
    )
    .unwrap();
    loop {
        match read_message(&mut reader).unwrap() {
            FramePacket::Frame(frame) if frame.height == 352 => break,
            FramePacket::Json(bytes) => assert!(!matches!(
                serde_json::from_slice::<PluginToHost>(&bytes).unwrap(),
                PluginToHost::Present { .. }
            )),
            _ => {}
        }
        assert!(Instant::now() < deadline);
    }
    // Keyboard navigation reaches the live Connections page without installing hooks.
    for key in ["tab", "tab", "enter"] {
        write_json(
            &mut socket,
            &HostToPlugin::Input {
                event: InputEvent::KeyDown { key: key.into() },
            },
        )
        .unwrap();
    }
    loop {
        if let FramePacket::Json(bytes) = read_message(&mut reader).unwrap()
            && matches!(
                serde_json::from_slice::<PluginToHost>(&bytes).unwrap(),
                PluginToHost::PreferredSize {
                    h: island_design::agent_pulse::SETTINGS_HEIGHT,
                    ..
                }
            )
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "keyboard navigation did not open Connections"
        );
    }
    write_json(
        &mut socket,
        &HostToPlugin::SurfaceResize {
            surface_id: 1,
            w: 340,
            h: island_design::agent_pulse::SETTINGS_HEIGHT,
            scale: 2.0,
        },
    )
    .unwrap();
    loop {
        if let FramePacket::Frame(frame) = read_message(&mut reader).unwrap()
            && frame.height == island_design::agent_pulse::SETTINGS_HEIGHT * 2
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Connections did not render at its requested size"
        );
    }
    // A new request returns from Connections to the prioritized session list.
    // No origin: this fixture must never activate the test runner's terminal.
    let task_id = "00000000-0000-4000-8000-000000000001";
    store::record(
        temp.path(),
        hooks::normalize(
            Provider::Codex,
            &serde_json::json!({
                "session_id": task_id, "hook_event_name": "PreToolUse",
                "tool_name": "request_user_input", "cwd": "/tmp/second"
            }),
            now_ms(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut new_attention = false;
    let mut new_size = false;
    let mut new_frame = false;
    while !new_attention || !new_size || !new_frame {
        assert!(
            Instant::now() < deadline,
            "new request did not return to the session list"
        );
        match read_message(&mut reader).unwrap() {
            FramePacket::Json(bytes) => {
                match serde_json::from_slice::<PluginToHost>(&bytes).unwrap() {
                    PluginToHost::Present {
                        priority: 90,
                        mode: island_plugin_api::PresentationMode::Hover,
                        ..
                    } => new_attention = true,
                    PluginToHost::PreferredSize { w, h: 140 } => {
                        new_size = true;
                        write_json(
                            &mut socket,
                            &HostToPlugin::SurfaceResize {
                                surface_id: 1,
                                w,
                                h: 140,
                                scale: 2.0,
                            },
                        )
                        .unwrap();
                    }
                    _ => {}
                }
            }
            FramePacket::Frame(frame) => new_frame |= frame.height == 280,
        }
    }
    // Repeated row clicks share one pending launch. A failed OS handoff stays
    // on the list, with ongoing frames and input, instead of dismissing it.
    for _ in 0..2 {
        for event in [
            InputEvent::PointerDown {
                x: 40.0,
                y: 50.0,
                button: 0,
            },
            InputEvent::PointerUp {
                x: 40.0,
                y: 50.0,
                button: 0,
            },
        ] {
            write_json(&mut socket, &HostToPlugin::Input { event }).unwrap();
        }
    }
    write_json(
        &mut socket,
        &HostToPlugin::Input {
            event: InputEvent::PointerMove { x: 40.0, y: 50.0 },
        },
    )
    .unwrap();
    let can_open_codex = navigation::codex_available();
    let mut launches = 0;
    let mut saw_barrier = false;
    let mut saw_reply_barrier = false;
    loop {
        assert!(
            Instant::now() < deadline,
            "session launch did not keep rendering"
        );
        match read_message(&mut reader).unwrap() {
            FramePacket::Frame(frame) => {
                assert_eq!(frame.height, 280, "launch replaced the list");
                if saw_barrier && (!can_open_codex || saw_reply_barrier) {
                    break;
                }
            }
            FramePacket::Json(bytes) => {
                match serde_json::from_slice::<PluginToHost>(&bytes).unwrap() {
                    PluginToHost::OpenUrl { url, request_id } => {
                        launches += 1;
                        assert_eq!(launches, 1, "duplicate click launched twice");
                        assert_eq!(url, format!("codex://threads/{task_id}"));
                        write_json(
                            &mut socket,
                            &HostToPlugin::OpenUrlResult {
                                request_id: request_id.expect("launch needs a reply"),
                                error: Some("The application could not open this session".into()),
                            },
                        )
                        .unwrap();
                        write_json(
                            &mut socket,
                            &HostToPlugin::Input {
                                event: InputEvent::PointerMove { x: 40.0, y: 50.0 },
                            },
                        )
                        .unwrap();
                    }
                    PluginToHost::SetCursor { .. } => {
                        if saw_barrier {
                            saw_reply_barrier = true;
                        }
                        saw_barrier = true;
                    }
                    PluginToHost::Dismiss
                    | PluginToHost::Present { .. }
                    | PluginToHost::PreferredSize { .. } => panic!("launch changed presentation"),
                    _ => {}
                }
            }
        }
    }
    assert_eq!(launches, usize::from(can_open_codex));
    assert!(child.0.try_wait().unwrap().is_none());
    // No config files are created simply by loading the plugin or inspecting connections.
    assert!(!temp.path().join("connected.json").exists());
    write_json(&mut socket, &HostToPlugin::Shutdown).unwrap();
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "plugin ignored shutdown");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child.0.wait().unwrap().success());
    drop(socket);
    drop(reader);
    drop(listener);
    assert!(TcpListener::bind(address).is_ok());
}
