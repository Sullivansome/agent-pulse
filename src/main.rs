use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use island_plugin_agent_pulse::{
    PLUGIN_ID, connections, default_data_dir, grok, hooks,
    model::{self, Provider, Status},
    navigation, now_ms,
    render::{self, Action, Page, Renderer, Ui},
    setup, store, terminal,
};
use island_plugin_api::{ExpandPhase, HostToPlugin, InputEvent, PluginToHost, PresentationMode};
use island_plugin_sdk::PluginClient;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(about = "Agent Pulse: local status for Claude Code, Codex, and Grok")]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Subcommand)]
enum Cmd {
    /// Receive a lifecycle hook on stdin. Always observational; never approves or blocks.
    Hook {
        #[arg(long, value_enum)]
        provider: Provider,
        #[arg(long, hide = true)]
        island_agent_pulse: bool,
    },
    /// Add hooks (or remove only Agent Pulse hooks), preserving and backing up existing config.
    Setup {
        #[arg(long, value_enum)]
        provider: Option<Provider>,
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Read the locally recorded session states without starting Island.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Render representative sample states for Market and visual QA.
    ExportPreviews { output: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.data_dir.unwrap_or_else(default_data_dir);
    match cli.command {
        Some(Cmd::Hook { provider, .. }) => {
            // Grok also discovers ~/.claude/settings.json. Its native registration owns reporting.
            if !(provider == Provider::Claude && std::env::var_os("GROK_SESSION_ID").is_some())
                && let Err(error) = receive_hook(provider, &dir)
            {
                eprintln!("Agent Pulse: {error:#}");
            }
            // Codex Stop requires valid JSON. An empty object never changes an agent decision.
            println!("{{}}");
            Ok(())
        }
        Some(Cmd::Setup {
            provider,
            remove,
            home,
        }) => {
            let use_env = home.is_none();
            let home = home
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                .context("HOME is not set")?;
            let providers = provider
                .map(|p| vec![p])
                .unwrap_or_else(|| Provider::ALL.to_vec());
            let paths = setup::configure(
                &home,
                &dir,
                &std::env::current_exe()?,
                &providers,
                remove,
                use_env,
            )?;
            for path in paths {
                println!("Updated {}", path.display());
            }
            if remove {
                println!(
                    "Agent Pulse hooks removed. Existing hooks and local status data were kept."
                );
            } else {
                println!(
                    "Hooks installed. Restart Claude Code and Grok. In Codex, review Agent Pulse in Settings → Hooks (desktop) or /hooks (CLI). No trust settings were changed."
                );
            }
            Ok(())
        }
        Some(Cmd::Status { json }) => {
            let now = now_ms();
            let mut state = store::load(&dir)?;
            if let Some(active) = grok::active_sessions() {
                grok::reconcile(&mut state.sessions, &active, now);
            }
            terminal::reconcile(&mut state.sessions);
            let sessions = state.visible(now);
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else if sessions.is_empty() {
                println!("No status signals yet. Run setup, then start an agent task.");
            } else {
                for s in sessions {
                    println!(
                        "{}\t{}\t{}\t{}",
                        s.provider.name(),
                        s.project,
                        s.status.label(),
                        s.id
                    );
                }
            }
            Ok(())
        }
        Some(Cmd::ExportPreviews { output }) => render::export(&output),
        None => match run(cli_data_override(&dir)) {
            // The SDK closes its writer as soon as it receives Shutdown. A final
            // frame can race that close; host disconnect is a normal exit.
            Err(error) if error.downcast_ref::<island_plugin_sdk::SdkError>().is_some_and(|e|
                matches!(e, island_plugin_sdk::SdkError::Io(io) if matches!(io.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::NotConnected))) => Ok(()),
            result => result,
        },
    }
}

// PluginClient's configured data directory is authoritative unless a caller gave
// an explicit override (primarily useful to isolated QA hosts).
fn cli_data_override(dir: &Path) -> Option<PathBuf> {
    std::env::args()
        .any(|a| a == "--data-dir" || a.starts_with("--data-dir="))
        .then(|| dir.to_path_buf())
}

fn receive_hook(provider: Provider, dir: &Path) -> Result<()> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 1024 * 1024, "hook input exceeds 1 MiB");
    let value = serde_json::from_slice(&bytes)?;
    if let Some(mut event) = hooks::normalize(provider, &value, now_ms()) {
        event.origin = navigation::capture(provider);
        event.terminal = terminal::capture(provider, &event.id);
        store::record(dir, event)?;
    }
    Ok(())
}

fn run(override_dir: Option<PathBuf>) -> Result<()> {
    let mut client = PluginClient::connect(PLUGIN_ID)?;
    let dir = override_dir.unwrap_or_else(|| client.data_dir.clone());
    store::private_dir(&dir)?;
    let renderer = Renderer::new();
    anyhow::ensure!(
        renderer.has_font(),
        "Agent Pulse needs Arial, DejaVu Sans, or Liberation Sans installed"
    );
    let mut ui = Ui {
        codex_available: navigation::codex_available(),
        ..Ui::default()
    };
    let (tx, rx) = mpsc::channel::<std::result::Result<String, String>>();
    let (mut w, mut h, mut scale) = (
        island_design::agent_pulse::WIDTH,
        island_design::agent_pulse::HEIGHT,
        2.0,
    );
    let mut last_render = Instant::now() - Duration::from_secs(2);
    let mut last_connection_check = Instant::now() - Duration::from_secs(5);
    let mut frame_id = 0;
    let mut signature = None;
    let mut message_at = Instant::now();
    let mut sessions = vec![];
    let mut selected_id: Option<(Provider, String)> = None;
    let mut first = true;
    let mut requested_size = None;
    let mut launch = navigation::LaunchState::default();
    loop {
        let mut dirty = false;
        for msg in client.drain() {
            let mut action = None;
            match msg {
                HostToPlugin::Shutdown => return Ok(()),
                HostToPlugin::SurfaceReady {
                    w: nw,
                    h: nh,
                    scale: sc,
                    ..
                }
                | HostToPlugin::SurfaceResize {
                    w: nw,
                    h: nh,
                    scale: sc,
                    ..
                } => {
                    w = nw;
                    h = nh;
                    scale = sc;
                    dirty = true;
                }
                HostToPlugin::ExpandState { phase, .. } => {
                    ui.expanded = matches!(phase, ExpandPhase::Expanded | ExpandPhase::Expanding);
                    if phase == ExpandPhase::Collapsed {
                        ui.page = Page::Sessions;
                        ui.focus = None;
                        ui.hover = None;
                        ui.list.end_drag();
                    }
                    dirty = true;
                }
                HostToPlugin::Focus { focused: false } => {
                    ui.focus = None;
                    ui.pressed = None;
                    ui.list.end_drag();
                    dirty = true;
                }
                HostToPlugin::OpenUrlResult { request_id, error } => {
                    if launch.finish(request_id) {
                        ui.error = error.is_some();
                        ui.message = error.unwrap_or_default();
                        message_at = Instant::now();
                        dirty = true;
                    }
                }
                HostToPlugin::Input { event } => {
                    dirty = true;
                    match event {
                        InputEvent::PointerMove { x, y } => {
                            ui.list.drag_to(y, sessions.len(), h);
                            ui.hover = render::hit(w, h, &sessions, &ui, x, y);
                            client.send(&PluginToHost::SetCursor {
                                name: if ui.hover.is_some() {
                                    "pointer"
                                } else {
                                    "default"
                                }
                                .into(),
                            })?;
                        }
                        InputEvent::PointerDown { x, y, button: 0 } => {
                            ui.pressed = render::hit(w, h, &sessions, &ui, x, y);
                            if ui.pressed == Some(Action::Scrollbar) {
                                ui.list.press_scrollbar(y, sessions.len(), h);
                                ui.pressed = None;
                            }
                            ui.focus = ui.pressed;
                        }
                        InputEvent::PointerUp { x, y, button: 0 } => {
                            let hit = render::hit(w, h, &sessions, &ui, x, y);
                            if !ui.list.dragging() && hit == ui.pressed {
                                action = hit;
                            }
                            ui.list.end_drag();
                            ui.pressed = None;
                        }
                        InputEvent::Scroll { x, y, dy, .. }
                            if !sessions.is_empty()
                                && ui.page == Page::Sessions
                                && h >= 64
                                && x.is_finite()
                                && x >= 0.0
                                && x < w as f32
                                && island_plugin_agent_pulse::list::Viewport::contains_y(h, y) =>
                        {
                            ui.list.scroll(dy, sessions.len(), h);
                            ui.pressed = None;
                            ui.hover = render::hit(w, h, &sessions, &ui, x, y);
                        }
                        InputEvent::KeyDown { key } => match key.as_str() {
                            "down" | "ArrowDown"
                                if !sessions.is_empty() && ui.page == Page::Sessions =>
                            {
                                action =
                                    Some(Action::Select((ui.selected + 1).min(sessions.len() - 1)))
                            }
                            "up" | "ArrowUp"
                                if !sessions.is_empty() && ui.page == Page::Sessions =>
                            {
                                action = Some(Action::Select(ui.selected.saturating_sub(1)))
                            }
                            "home" | "Home"
                                if !sessions.is_empty() && ui.page == Page::Sessions =>
                            {
                                action = Some(Action::Select(0))
                            }
                            "end" | "End" if !sessions.is_empty() && ui.page == Page::Sessions => {
                                action = Some(Action::Select(sessions.len() - 1))
                            }
                            "pageup" | "PageUp" | "page_up" | "pagedown" | "PageDown"
                            | "page_down"
                                if !sessions.is_empty() && ui.page == Page::Sessions =>
                            {
                                let step = (island_plugin_agent_pulse::list::Viewport::height(h)
                                    / island_design::agent_pulse::ROW)
                                    .floor()
                                    .max(1.0) as usize;
                                action = Some(Action::Select(
                                    if key.eq_ignore_ascii_case("pageup") || key == "page_up" {
                                        ui.selected.saturating_sub(step)
                                    } else {
                                        ui.selected.saturating_add(step).min(sessions.len() - 1)
                                    },
                                ));
                            }
                            "tab" | "Tab" => {
                                let controls = render::actions(&sessions, &ui);
                                let next = ui
                                    .focus
                                    .and_then(|f| controls.iter().position(|a| *a == f))
                                    .map(|i| (i + 1) % controls.len())
                                    .unwrap_or(0);
                                ui.focus = Some(controls[next]);
                            }
                            "enter" | "Enter" | "return" | "space" | " " => action = ui.focus,
                            "escape" | "Escape" if ui.page != Page::Sessions => {
                                action = Some(Action::Back)
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                _ => {}
            }
            match action {
                Some(Action::Select(index)) => {
                    ui.selected = index;
                    ui.list.reveal(index, sessions.len(), h);
                    ui.focus = Some(Action::Row(index));
                    selected_id = sessions
                        .get(index)
                        .map(|s: &model::Session| (s.provider, s.id.clone()));
                }
                Some(Action::Menu) => {
                    ui.page = Page::Menu;
                    ui.focus = Some(Action::Back);
                    ui.hover = None;
                }
                Some(Action::Back) => {
                    ui.page = Page::Sessions;
                    ui.focus = Some(Action::Menu);
                    ui.hover = None;
                }
                Some(Action::Settings) => {
                    last_connection_check = Instant::now() - Duration::from_secs(5);
                    ui.page = Page::Settings;
                    ui.focus = Some(Action::Back);
                    ui.hover = None;
                }
                Some(Action::Row(_)) | Some(Action::Open) => {
                    if !client.permissions.iter().any(|p| p == "shell:open-url") {
                        ui.message = "Opening sessions is not permitted".into();
                        ui.error = true;
                        message_at = Instant::now();
                        continue;
                    }
                    let Some(request_id) = launch.begin(Instant::now()) else {
                        continue;
                    };
                    if let Some(Action::Row(index)) = action {
                        ui.selected = index;
                        selected_id = sessions.get(index).map(|s| (s.provider, s.id.clone()));
                    }
                    if let Some(session) = sessions.get(ui.selected) {
                        match navigation::target(session, ui.codex_available) {
                            navigation::Target::Url(url) => {
                                ui.message = "Opening session…".into();
                                ui.error = false;
                                message_at = Instant::now();
                                client.send(&PluginToHost::OpenUrl {
                                    url,
                                    request_id: Some(request_id),
                                })?
                            }
                            navigation::Target::Application(origin) => {
                                launch.finish(request_id);
                                if let Err(error) = navigation::open(origin) {
                                    ui.message = error.to_string();
                                    ui.error = true;
                                    message_at = Instant::now();
                                } else {
                                    ui.message.clear();
                                    ui.error = false;
                                }
                            }
                            navigation::Target::Unavailable => {
                                launch.finish(request_id);
                                ui.message = "Original window unavailable".into();
                                ui.error = true;
                                message_at = Instant::now();
                            }
                        }
                    } else {
                        launch.finish(request_id);
                    }
                }
                Some(Action::Setup)
                    if !ui.loading && ui.connections.configured() && ui.page == Page::Sessions =>
                {
                    ui.page = Page::Settings;
                    ui.focus = Some(Action::Back);
                }
                Some(Action::Setup) if !ui.loading => {
                    ui.page = Page::Settings;
                    ui.loading = true;
                    ui.error = false;
                    ui.setup_error = false;
                    let dir = dir.clone();
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let result = (|| -> Result<String> {
                            let home = std::env::var_os("HOME").context("HOME not set")?;
                            setup::configure(
                                Path::new(&home),
                                &dir,
                                &std::env::current_exe()?,
                                &Provider::ALL,
                                false,
                                true,
                            )?;
                            Ok("Hooks installed".into())
                        })();
                        let _ = tx.send(result.map_err(|e| format!("{e:#}")));
                    });
                }
                Some(Action::Copy) if !ui.loading => {
                    if let Some(s) = sessions.get(ui.selected) {
                        let flag = if s.provider == Provider::Codex {
                            "resume"
                        } else {
                            "--resume"
                        };
                        let command =
                            format!("{} {flag} {}", s.provider.key(), setup::quote(&s.id));
                        match copy(&command) {
                            Ok(()) => {
                                ui.message = "Copied".into();
                                ui.error = false;
                            }
                            Err(_) => {
                                ui.message = "Copy failed".into();
                                ui.error = true;
                            }
                        }
                        message_at = Instant::now();
                    }
                }
                _ => {}
            }
        }
        if launch.timed_out(Instant::now()) {
            ui.message = "Opening timed out; try again".into();
            ui.error = true;
            message_at = Instant::now();
            dirty = true;
        }
        while let Ok(result) = rx.try_recv() {
            ui.loading = false;
            last_connection_check = Instant::now() - Duration::from_secs(5);
            match result {
                Ok(message) => {
                    ui.message = message;
                    ui.error = false;
                }
                Err(error) => {
                    let _ = client.log(island_plugin_api::LogLevel::Error, error);
                    ui.message = "Connect failed; retry".into();
                    ui.error = true;
                    ui.setup_error = true;
                }
            }
            message_at = Instant::now();
            dirty = true;
        }
        if !ui.message.is_empty() && message_at.elapsed() > Duration::from_secs(10) {
            ui.message.clear();
            dirty = true;
        }
        if dirty
            || last_render.elapsed()
                >= if ui.expanded && sessions.iter().any(|s| s.status == Status::Working) {
                    Duration::from_millis(120)
                } else {
                    Duration::from_secs(1)
                }
        {
            let now = now_ms();
            match store::load(&dir) {
                Ok(mut state) => {
                    ui.connections.update_activity(Some(&state.sessions));
                    if let Some(active) = grok::active_sessions() {
                        grok::reconcile(&mut state.sessions, &active, now);
                        for session in &mut state.sessions {
                            if session.provider == Provider::Grok
                                && session.origin.is_none()
                                && let Some(entry) =
                                    active.iter().find(|a| a.session_id == session.id)
                            {
                                session.origin = navigation::for_process(entry.pid);
                            }
                        }
                    }
                    terminal::reconcile(&mut state.sessions);
                    let next_sessions = state.visible(now);
                    let viewport_height = if ui.page == Page::Sessions && h >= 64 {
                        h
                    } else {
                        render::preferred_height(next_sessions.len(), Page::Sessions)
                    };
                    ui.reconcile_sessions(&sessions, &next_sessions, viewport_height);
                    sessions = next_sessions;
                }
                Err(error) => {
                    ui.message = "Status unavailable".into();
                    ui.error = true;
                    let _ = client.log(island_plugin_api::LogLevel::Warn, error.to_string());
                    sessions.clear();
                    ui.connections.update_activity(None);
                }
            }
            if last_connection_check.elapsed() >= Duration::from_secs(5) {
                ui.connections.installation = std::env::var_os("HOME")
                    .map(|home| connections::inspect(Path::new(&home), &dir, true))
                    .unwrap_or([connections::Installation::Unavailable; 3]);
                last_connection_check = Instant::now();
            }
            if let Some((provider, ref id)) = selected_id {
                ui.selected = sessions
                    .iter()
                    .position(|s| s.provider == provider && &s.id == id)
                    .unwrap_or(0);
            }
            ui.selected = ui.selected.min(sessions.len().saturating_sub(1));
            let next = model::presentation(&sessions, now);
            let next_signature = next.as_ref().map(|(_, _, sig)| sig.clone());
            let presenting = next_signature != signature;
            if presenting {
                if next
                    .as_ref()
                    .is_some_and(|(priority, _, _)| *priority == 90)
                {
                    ui.selected = 0;
                    ui.list.reset();
                    selected_id = sessions.first().map(|s| (s.provider, s.id.clone()));
                    ui.page = Page::Sessions;
                    ui.focus = None;
                    ui.hover = None;
                    ui.pressed = None;
                }
                match &next {
                    Some((priority, sticky, _)) => client.present_with_mode(
                        *priority,
                        if *sticky { None } else { Some(12_000) },
                        *sticky,
                        Some("Agent status changed".into()),
                        model::presentation_mode(*priority),
                    )?,
                    None => client.dismiss()?,
                }
                signature = next_signature;
            }
            if first && next.is_none() {
                client.present_with_mode(
                    5,
                    None,
                    true,
                    Some("Agent Pulse ready".into()),
                    PresentationMode::Hover,
                )?;
            }
            first = false;
            // Hide is sticky until the next significant lifecycle change. Frames alone do not present.
            let primary = sessions.first();
            client.send(&PluginToHost::SetCollapsedMeta {
                leading: Some(
                    primary
                        .map(|s| s.provider.name())
                        .unwrap_or("Agent Pulse")
                        .into(),
                ),
                trailing: Some(render::summary(&sessions)),
                icon: primary.map(|s| renderer.collapsed_icon(s.provider)),
                dot: Some(
                    match primary.map(|s| (s.status, s.needs_attention())) {
                        Some((_, true)) => "attention",
                        Some((Status::Error, _)) => "error",
                        Some((Status::Ready, _)) => "success",
                        _ => "notify",
                    }
                    .into(),
                ),
            })?;
            let size = (
                island_design::agent_pulse::WIDTH,
                render::preferred_height(sessions.len(), ui.page),
            );
            if presenting || requested_size != Some(size) {
                client.preferred_size(size.0, size.1)?;
                requested_size = Some(size);
            }
            let mut frame = renderer.frame(w, h, scale, &sessions, &ui, now);
            frame_id += 1;
            frame.frame_id = frame_id;
            client.submit_frame(frame)?;
            last_render = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(33));
    }
}

fn copy(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let candidates: Vec<(&str, Vec<&str>)> = vec![("pbcopy", vec![])];
    #[cfg(not(target_os = "macos"))]
    let candidates = vec![
        ("wl-copy", vec![]),
        ("xclip", vec!["-selection", "clipboard"]),
    ];
    for (program, args) in candidates {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                break;
            }
            if start.elapsed() > Duration::from_millis(500) {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    anyhow::bail!("No clipboard service available")
}
