//! Launch origins are local routing metadata, independent of the renderer.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct LaunchState {
    sequence: u64,
    pending: Option<(u64, Instant)>,
    last: Option<Instant>,
}

impl LaunchState {
    pub fn begin(&mut self, now: Instant) -> Option<u64> {
        if self.pending.is_some()
            || self
                .last
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(600))
        {
            return None;
        }
        self.sequence += 1;
        self.pending = Some((self.sequence, now));
        self.last = Some(now);
        Some(self.sequence)
    }
    pub fn finish(&mut self, request_id: u64) -> bool {
        if self.pending.is_some_and(|(id, _)| id == request_id) {
            self.pending = None;
            true
        } else {
            false
        }
    }
    pub fn timed_out(&mut self, now: Instant) -> bool {
        if self
            .pending
            .is_some_and(|(_, at)| now.saturating_duration_since(at) > Duration::from_secs(4))
        {
            self.pending = None;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub pid: u32,
    pub started_sec: u64,
    pub started_usec: u64,
    pub bundle_id: String,
    pub name: String,
}

pub fn capture(provider: crate::model::Provider) -> Option<Origin> {
    #[cfg(target_os = "macos")]
    {
        platform::capture(provider)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = provider;
        None
    }
}

pub enum Target<'a> {
    Url(String),
    Application(&'a Origin),
    Unavailable,
}

/// UUID-only task route; never accept commands or query parameters from hook data.
pub fn codex_url(session: &crate::model::Session) -> Option<String> {
    let url = format!("codex://threads/{}", session.id);
    (session.provider == crate::model::Provider::Codex
        && island_plugin_api::is_codex_thread_url(&url))
    .then_some(url)
}

pub fn target(session: &crate::model::Session, codex_available: bool) -> Target<'_> {
    // A CLI in a terminal should return to that terminal, even if Codex desktop is installed.
    if let Some(origin) = &session.origin
        && !origin.bundle_id.starts_with("com.openai.")
    {
        return Target::Application(origin);
    }
    if codex_available && let Some(url) = codex_url(session) {
        return Target::Url(url);
    }
    session
        .origin
        .as_ref()
        .map(Target::Application)
        .unwrap_or(Target::Unavailable)
}

#[cfg(target_os = "macos")]
#[path = "navigation_macos.rs"]
mod platform;
#[cfg(target_os = "macos")]
pub use platform::{codex_available, for_process, open};

#[cfg(not(target_os = "macos"))]
pub fn for_process(_: u32) -> Option<Origin> {
    None
}
#[cfg(not(target_os = "macos"))]
pub fn codex_available() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
pub fn open(_: &Origin) -> anyhow::Result<()> {
    anyhow::bail!("Opening the original terminal is not supported on this platform")
}
