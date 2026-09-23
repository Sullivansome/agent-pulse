//! Real PTYs: closing one terminal must retire its agent without hiding another.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use island_plugin_agent_pulse::{model::Sessions, now_ms, process, store, terminal};
use std::{
    fs::File,
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Terminal {
    master: Option<OwnedFd>,
    child: Child,
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.master.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Terminal {
    fn start(binary: &Path, data: &Path, id: &str) -> Self {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let mut command = Command::new(binary);
        command
            .args(["--exact", "fixture_agent", "--ignored", "--nocapture"])
            .env("ISLAND_TERMINAL_TEST_DATA", data)
            .env("ISLAND_TERMINAL_TEST_ID", id)
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                Ok(())
            });
        }
        Self {
            master: Some(master),
            child: command.spawn().unwrap(),
        }
    }
}

#[test]
#[ignore = "child fixture launched only by the PTY integration test"]
fn fixture_agent() {
    let data = std::env::var("ISLAND_TERMINAL_TEST_DATA").unwrap();
    let id = std::env::var("ISLAND_TERMINAL_TEST_ID").unwrap();
    let mut hook = Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
        .args(["hook", "--provider", "codex", "--data-dir", &data])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    hook.stdin.take().unwrap().write_all(
        serde_json::json!({"session_id": id, "hook_event_name": "UserPromptSubmit", "cwd": "/fixture/project"}).to_string().as_bytes()
    ).unwrap();
    assert!(hook.wait().unwrap().success());
    std::thread::sleep(Duration::from_secs(20));
}

#[test]
fn closing_a_pty_removes_its_row_while_another_terminal_keeps_working() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("codex");
    std::fs::copy(std::env::current_exe().unwrap(), &binary).unwrap();
    let data = temp.path().join("data");
    let mut first = Terminal::start(&binary, &data, "terminal-a");
    let second = Terminal::start(&binary, &data, "terminal-b");
    let deadline = Instant::now() + Duration::from_secs(8);
    let recorded = loop {
        let state = store::load(&data).unwrap();
        if state.sessions.len() == 2 && state.sessions.iter().all(|s| s.terminal.is_some()) {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "hook did not capture both terminal owners"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    for session in &recorded.sessions {
        let owner = session.terminal.as_ref().unwrap();
        assert!(owner.pid == first.child.id() || owner.pid == second.child.id());
        assert_eq!(process::is_alive(owner), Some(true));
        let mut recycled = owner.clone();
        recycled.started.push_str("-different-birth");
        assert_eq!(
            process::is_alive(&recycled),
            Some(false),
            "PID reuse must not keep an old row alive"
        );
    }
    first.master.take(); // A real terminal hangup, not a fabricated SessionEnd hook.
    loop {
        if first.child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "agent was not reaped after terminal hangup"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut state: Sessions = store::load(&data).unwrap();
    terminal::reconcile(&mut state.sessions);
    let visible = state.visible(now_ms());
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "terminal-b");
    assert!(process::is_alive(visible[0].terminal.as_ref().unwrap()).unwrap());

    // The CLI follows the same lifetime policy as the rendered list.
    let result = Command::new(env!("CARGO_BIN_EXE_island-plugin-agent-pulse"))
        .args(["status", "--json", "--data-dir"])
        .arg(&data)
        .output()
        .unwrap();
    assert!(result.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "terminal-b");
}
