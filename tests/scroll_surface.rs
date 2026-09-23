//! Actual plugin transport: wheel, scrollbar, keyboard and scrolled row launch.
use island_plugin_agent_pulse::{
    model::{Provider, Sessions, Status},
    navigation, now_ms,
    render::{self, Action, Renderer, Ui},
};
use island_plugin_api::{
    FramePacket, HostToPlugin, InputEvent, PluginToHost, SurfaceFrame, read_message, write_json,
};
use std::{
    io::BufReader,
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Plugin(Child);
impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn frame(reader: &mut BufReader<TcpStream>, expected: &SurfaceFrame, initial: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < deadline,
            "scrolled frame did not match the expected rows"
        );
        match read_message(reader).unwrap() {
            FramePacket::Frame(actual) if actual.pixels == expected.pixels => return,
            FramePacket::Json(bytes) if !initial => assert!(
                !matches!(
                    serde_json::from_slice::<PluginToHost>(&bytes).unwrap(),
                    PluginToHost::Present { .. }
                        | PluginToHost::Dismiss
                        | PluginToHost::PreferredSize { .. }
                ),
                "scrolling must not change presentation"
            ),
            _ => {}
        }
    }
}
fn input(socket: &mut TcpStream, event: InputEvent) {
    write_json(socket, &HostToPlugin::Input { event }).unwrap();
}

#[test]
fn real_plugin_scrolls_all_sessions_and_clicks_the_correct_last_row() {
    let temp = tempfile::tempdir().unwrap();
    let now = now_ms();
    let sessions = (0..8)
        .map(|i| {
            let mut session = render::samples(now).remove(2);
            session.provider = Provider::Codex;
            session.id = format!("00000000-0000-4000-8000-{i:012}");
            session.project = format!("project-{i}");
            session.updated_ms = now - i;
            session.signal_ms = now;
            session.status = Status::Ready;
            session
        })
        .collect();
    let state = Sessions { sessions };
    std::fs::write(
        temp.path().join("sessions.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let sessions = state.visible(now);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Plugin(
        Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
            .env(
                "ISLAND_ENDPOINT",
                format!("tcp:{}", listener.local_addr().unwrap()),
            )
            .env("ISLAND_PLUGIN_TOKEN", "scroll-test")
            .env("ISLAND_PLUGIN_ID", island_plugin_agent_pulse::PLUGIN_ID)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut socket, _) = loop {
        if let Ok(stream) = listener.accept() {
            break stream;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    write_json(&mut socket, &HostToPlugin::hello("test", "test")).unwrap();
    let FramePacket::Json(hello) = read_message(&mut reader).unwrap() else {
        panic!("handshake expected")
    };
    assert!(
        matches!(serde_json::from_slice::<PluginToHost>(&hello).unwrap(), PluginToHost::HelloOk { token, .. } if token == "scroll-test")
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
            h: 192,
            scale: 2.0,
        },
    )
    .unwrap();
    let renderer = Renderer::new();
    let mut ui = Ui::default();
    let expected = |ui: &Ui| renderer.frame(340, 192, 2.0, &sessions, ui, now);
    frame(&mut reader, &expected(&ui), true);

    input(
        &mut socket,
        InputEvent::Scroll {
            x: 100.0,
            y: 100.0,
            dx: 0.0,
            dy: 260.0,
        },
    );
    ui.list.offset = 260.0;
    ui.hover = Some(Action::Row(6));
    frame(&mut reader, &expected(&ui), false);
    input(&mut socket, InputEvent::KeyDown { key: "Home".into() });
    ui.list.reset();
    ui.focus = Some(Action::Row(0));
    frame(&mut reader, &expected(&ui), false);

    input(
        &mut socket,
        InputEvent::PointerDown {
            x: 338.0,
            y: 35.0,
            button: 0,
        },
    );
    input(&mut socket, InputEvent::PointerMove { x: 338.0, y: 184.0 });
    input(
        &mut socket,
        InputEvent::PointerUp {
            x: 338.0,
            y: 184.0,
            button: 0,
        },
    );
    ui.list.offset = 260.0;
    ui.focus = None;
    ui.hover = None; // bottom padding is outside the viewport
    frame(&mut reader, &expected(&ui), false);

    input(
        &mut socket,
        InputEvent::PointerDown {
            x: 40.0,
            y: 170.0,
            button: 0,
        },
    );
    input(
        &mut socket,
        InputEvent::PointerUp {
            x: 40.0,
            y: 170.0,
            button: 0,
        },
    );
    if navigation::codex_available() {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline);
            if let FramePacket::Json(bytes) = read_message(&mut reader).unwrap()
                && let PluginToHost::OpenUrl { url, .. } =
                    serde_json::from_slice::<PluginToHost>(&bytes).unwrap()
            {
                assert_eq!(url, format!("codex://threads/{}", sessions[7].id));
                break;
            }
        }
    } else {
        ui.selected = 7;
        ui.focus = Some(Action::Row(7));
        ui.error = true;
        ui.message = "Original window unavailable".into();
        frame(&mut reader, &expected(&ui), false);
    }
    write_json(&mut socket, &HostToPlugin::Shutdown).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child.0.wait().unwrap().success());
}
