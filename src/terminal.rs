//! Terminal lifetime is separate from the activity of an individual agent turn.
use crate::{
    model::{Provider, Session, Status},
    process::{self, TerminalProcess},
};

pub fn capture(provider: Provider, session_id: &str) -> Option<TerminalProcess> {
    let registered = (provider == Provider::Grok)
        .then(crate::grok::active_sessions)
        .flatten()
        .and_then(|active| active.into_iter().find(|s| s.session_id == session_id))
        .map(|s| s.pid);
    capture_from(std::process::id(), provider, registered)
}

fn capture_from(
    mut pid: u32,
    provider: Provider,
    registered: Option<u32>,
) -> Option<TerminalProcess> {
    // Start above the short-lived receiver. The provider's process (not the
    // terminal app or hook shell) owns this session's lifetime.
    pid = process::inspect(pid).ok().flatten()?.parent;
    for _ in 0..32 {
        if pid <= 1 {
            break;
        }
        let info = process::inspect(pid).ok().flatten()?;
        let named =
            info.name == provider.key() || info.name.starts_with(&format!("{}-", provider.key()));
        if named || registered == Some(pid) {
            return process::terminal_process(pid);
        }
        if info.parent == pid {
            break;
        }
        pid = info.parent;
    }
    None
}

pub fn reconcile(sessions: &mut [Session]) {
    reconcile_with(sessions, process::is_alive);
}

fn reconcile_with(
    sessions: &mut [Session],
    mut alive: impl FnMut(&TerminalProcess) -> Option<bool>,
) {
    for session in sessions {
        if session
            .terminal
            .as_ref()
            .is_some_and(|owner| alive(owner) == Some(false))
        {
            session.status = Status::Ended;
            session.detail = "Terminal session closed".into();
            session.pending_requests.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Sessions, render};

    #[test]
    fn closed_terminal_removes_only_its_session_and_read_errors_keep_rows() {
        let mut sessions = render::samples(100_000);
        for (i, session) in sessions.iter_mut().enumerate().take(2) {
            session.terminal = Some(TerminalProcess {
                pid: 100 + i as u32,
                started: "fixture".into(),
                terminal: 1,
            });
        }
        let original = sessions.clone();
        reconcile_with(&mut sessions, |_| None);
        assert_eq!(sessions, original, "inspection failure is not closure");
        reconcile_with(&mut sessions, |p| Some(p.pid != 100));
        let visible = Sessions { sessions }.visible(100_001);
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|s| s.id != original[0].id));
        assert!(visible.iter().any(|s| s.id == original[1].id));
        assert!(
            visible.iter().any(|s| s.terminal.is_none()),
            "desktop sessions remain"
        );
    }
}
