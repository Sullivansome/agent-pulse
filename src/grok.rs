//! Grok's small process registry supplies liveness, never activity or conversation content.
use crate::model::{Provider, Session, Status};
use serde::Deserialize;
use std::io::Read;

#[derive(Debug, Deserialize)]
pub struct ActiveSession {
    pub session_id: String,
    pub pid: u32,
}

pub fn active_sessions() -> Option<Vec<ActiveSession>> {
    let home = std::env::var_os("GROK_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".grok")))?;
    let mut bytes = vec![];
    std::fs::File::open(home.join("active_sessions.json"))
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub fn reconcile(sessions: &mut [Session], active: &[ActiveSession], now: u64) {
    for session in sessions {
        if session.provider == Provider::Grok
            && session.status != Status::Ended
            // SessionStart and registry registration can race. Never erase a fresh hook.
            && now.saturating_sub(session.updated_ms) > 30_000
            && !active.iter().any(|a| a.session_id == session.id)
        {
            session.status = Status::Ended;
            session.detail = "Session no longer listed by Grok".into();
            session.origin = None;
            session.pending_requests.clear();
        }
    }
}
