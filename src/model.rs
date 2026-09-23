//! Toolkit-independent state. Only explicit lifecycle signals determine activity.
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub const STALE_MS: u64 = 10 * 60 * 1000;
pub const RETAIN_MS: u64 = 24 * 60 * 60 * 1000;
pub const MAX_SESSIONS: usize = 128;
const MAX_PENDING_REQUESTS: usize = 16;

/// Only call IDs and one-way input digests are retained, never tool arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolIdentity {
    pub call_id: Option<String>,
    pub fingerprint: Option<String>,
}

impl ToolIdentity {
    fn matches(&self, other: &Self) -> bool {
        match (&self.call_id, &other.call_id) {
            (Some(a), Some(b)) => a == b,
            _ => self.fingerprint.is_some() && self.fingerprint == other.fingerprint,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingRequest {
    pub tool: ToolIdentity,
    pub detail: String,
    #[serde(default)]
    pub requested_ms: u64,
    #[serde(default)]
    pub question: bool,
    /// An asynchronous question tool returns before the person has answered.
    pub awaiting_user_reply: bool,
}

impl PendingRequest {
    fn is_question(&self) -> bool {
        // Preserve questions recorded before the explicit kind was stored.
        self.question || self.awaiting_user_reply || self.detail == "Question to answer"
    }
}

#[derive(Debug, Clone)]
pub enum WaitUpdate {
    Preserve,
    Request(PendingRequest),
    ToolFinished(ToolIdentity),
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Grok,
}

impl Provider {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Grok];
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idle,
    Working,
    Waiting,
    Ready,
    Error,
    Interrupted,
    Ended,
    Unknown,
}

impl Status {
    fn settles_turn(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Error | Self::Interrupted | Self::Ended
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready for a prompt",
            Self::Working => "Working",
            Self::Waiting => "Needs you",
            Self::Ready => "Turn ended",
            Self::Error => "Turn failed",
            Self::Interrupted => "Interrupted",
            Self::Ended => "Session closed",
            Self::Unknown => "No recent signal",
        }
    }
    pub fn rank(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Error => 1,
            Self::Working => 2,
            Self::Ready => 3,
            Self::Interrupted => 4,
            Self::Idle => 5,
            Self::Unknown => 6,
            Self::Ended => 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub provider: Provider,
    pub id: String,
    pub project: String,
    pub status: Status,
    pub detail: String,
    pub turn_id: Option<String>,
    pub turn_started_ms: u64,
    pub updated_ms: u64,
    pub signal_ms: u64,
    #[serde(default)]
    pub last_working_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::navigation::Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::process::TerminalProcess>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_requests: Vec<PendingRequest>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub provider: Provider,
    pub id: String,
    pub project: String,
    pub status: Status,
    pub detail: String,
    pub turn_id: Option<String>,
    pub starts_turn: bool,
    pub at_ms: u64,
    pub origin: Option<crate::navigation::Origin>,
    pub terminal: Option<crate::process::TerminalProcess>,
    pub wait_update: WaitUpdate,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sessions {
    pub sessions: Vec<Session>,
}

impl Sessions {
    pub fn apply(&mut self, event: Event) {
        let pos = self
            .sessions
            .iter()
            .position(|s| s.provider == event.provider && s.id == event.id);
        if let Some(i) = pos {
            let old = &self.sessions[i];
            let same_turn = event.turn_id.is_some() && event.turn_id == old.turn_id;
            // Completion wins over late tool/question reports for that Codex
            // turn. A new UserPromptSubmit (including continuation) reopens it.
            if event.provider == Provider::Codex
                && same_turn
                && old.status.settles_turn()
                && !event.starts_turn
                && !event.status.settles_turn()
            {
                return;
            }
            let settles_current =
                event.provider == Provider::Codex && same_turn && event.status.settles_turn();
            // Separate hook processes can acquire the store lock out of order.
            // Merge request changes within a known, still-active turn, but never
            // resurrect a request after a newer turn-end signal.
            let merge_wait = event.turn_id.is_some()
                && event.turn_id == old.turn_id
                && matches!(old.status, Status::Working | Status::Waiting)
                && matches!(
                    event.wait_update,
                    WaitUpdate::Request(_) | WaitUpdate::ToolFinished(_)
                );
            // Delayed reports from an older Grok/Codex turn cannot finish a newer turn.
            if (event.at_ms < old.updated_ms && !merge_wait && !settles_current)
                || (!event.starts_turn
                    && event.turn_id.is_some()
                    && old.turn_id.is_some()
                    && event.turn_id != old.turn_id)
            {
                return;
            }
            let s = &mut self.sessions[i];
            // Legacy Codex records have no tool identity. Preserve their wait
            // until a turn boundary instead of guessing which tool answered it.
            if s.provider == Provider::Codex
                && s.status == Status::Waiting
                && s.pending_requests.is_empty()
                && matches!(
                    event.wait_update,
                    WaitUpdate::Preserve | WaitUpdate::ToolFinished(_)
                )
            {
                s.pending_requests.push(PendingRequest {
                    tool: ToolIdentity::default(),
                    detail: s.detail.clone(),
                    requested_ms: s.signal_ms,
                    question: s.detail == "Question to answer",
                    awaiting_user_reply: false,
                });
            }
            if event.status == Status::Working
                && !matches!(event.wait_update, WaitUpdate::Request(_))
            {
                s.last_working_ms = s.last_working_ms.max(event.at_ms);
            } else if event.starts_turn || event.status.settles_turn() {
                s.last_working_ms = 0;
            }
            // A parallel tool finishing does not answer a different tool's prompt.
            s.update_waits(event.wait_update);
            let (status, detail) = s.effective_activity(event.status, event.detail);
            let changed = s.status != status || event.starts_turn;
            s.status = status;
            s.detail = detail;
            if !event.project.is_empty() {
                s.project = event.project;
            }
            if event.starts_turn {
                s.turn_started_ms = event.at_ms;
            }
            if event.turn_id.is_some() || event.starts_turn {
                s.turn_id = event.turn_id;
            }
            if changed {
                s.signal_ms = event.at_ms;
            }
            s.updated_ms = s.updated_ms.max(event.at_ms);
            if event.origin.is_some() {
                s.origin = event.origin;
            }
            if event.terminal.is_some() || event.starts_turn {
                s.terminal = event.terminal;
            }
        } else {
            let mut session = Session {
                provider: event.provider,
                id: event.id,
                project: event.project,
                status: event.status,
                detail: event.detail,
                turn_id: event.turn_id,
                turn_started_ms: event.at_ms,
                updated_ms: event.at_ms,
                signal_ms: event.at_ms,
                last_working_ms: if event.status == Status::Working
                    && !matches!(event.wait_update, WaitUpdate::Request(_))
                {
                    event.at_ms
                } else {
                    0
                },
                origin: event.origin,
                terminal: event.terminal,
                pending_requests: vec![],
            };
            session.update_waits(event.wait_update);
            self.sessions.push(session);
        }
        self.sessions
            .sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
        self.sessions.truncate(MAX_SESSIONS);
    }

    pub fn visible(&self, now: u64) -> Vec<Session> {
        let mut sessions: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| s.status != Status::Ended && now.saturating_sub(s.updated_ms) < RETAIN_MS)
            .map(|s| {
                let mut s = s.clone();
                // Reconcile records written by older receivers immediately,
                // without requiring a new hook or mutating the live store.
                if s.provider == Provider::Codex
                    && matches!(s.status, Status::Working | Status::Waiting)
                {
                    (s.status, s.detail) = s.effective_activity(s.status, s.detail.clone());
                }
                // Human response time is not evidence that a request was resolved.
                if s.status == Status::Working && now.saturating_sub(s.updated_ms) > STALE_MS {
                    s.status = Status::Unknown;
                    s.detail = "Last signal over 10m ago".into();
                }
                s
            })
            .collect();
        sessions.sort_by_key(|s| {
            (
                !s.needs_attention(),
                s.status.rank(),
                std::cmp::Reverse(s.updated_ms),
            )
        });
        sessions
    }
}

impl Session {
    pub fn needs_attention(&self) -> bool {
        self.status == Status::Waiting
    }

    pub fn checking_permissions(&self) -> bool {
        self.status == Status::Working && self.detail == "Checking permissions"
    }

    pub fn activity_label(&self) -> &str {
        match self.status {
            Status::Waiting if !self.detail.is_empty() => &self.detail,
            Status::Working if self.checking_permissions() => "Checking permissions",
            _ => self.status.label(),
        }
    }

    fn update_waits(&mut self, update: WaitUpdate) {
        match update {
            WaitUpdate::Clear => self.pending_requests.clear(),
            WaitUpdate::Preserve => {}
            WaitUpdate::Request(request) => {
                if let Some(existing) = self.pending_requests.iter_mut().find(|p| {
                    request.tool.call_id.is_some() && p.tool.call_id == request.tool.call_id
                }) {
                    *existing = request;
                } else if self.pending_requests.len() < MAX_PENDING_REQUESTS {
                    self.pending_requests.push(request);
                } else {
                    // Overflow preserves real questions, but must not turn a
                    // burst of automatic permission reviews into human input.
                    let question = request.is_question()
                        || self.pending_requests[MAX_PENDING_REQUESTS - 1].is_question();
                    self.pending_requests[MAX_PENDING_REQUESTS - 1] = PendingRequest {
                        tool: ToolIdentity::default(),
                        detail: "Requests pending".into(),
                        requested_ms: request.requested_ms,
                        question,
                        awaiting_user_reply: false,
                    };
                }
            }
            WaitUpdate::ToolFinished(tool) => {
                if let Some(index) = self
                    .pending_requests
                    .iter()
                    .position(|p| !p.awaiting_user_reply && p.tool.matches(&tool))
                {
                    self.pending_requests.remove(index);
                }
            }
        }
    }

    fn effective_activity(&self, status: Status, detail: String) -> (Status, String) {
        if let Some(question) = self.pending_requests.iter().find(|p| p.is_question()) {
            return (Status::Waiting, question.detail.clone());
        }
        if let Some(pending) = self.pending_requests.iter().max_by_key(|p| p.requested_ms) {
            // Keep correlation metadata without treating missing completion as
            // evidence of a human prompt. Auto-review emits the same hook.
            if self.last_working_ms > pending.requested_ms {
                (Status::Working, "Thinking".into())
            } else {
                (Status::Working, "Checking permissions".into())
            }
        } else if self.provider == Provider::Codex
            && status == Status::Waiting
            && detail == "Permission requested"
        {
            // Pre-correlation stores only retained this label.
            (Status::Working, "Checking permissions".into())
        } else {
            (status, detail)
        }
    }
}

pub fn presentation_mode(_priority: u8) -> island_plugin_api::PresentationMode {
    island_plugin_api::PresentationMode::Hover
}

/// A change of tool name does not repeatedly expand a manually collapsed island.
pub fn presentation(sessions: &[Session], now: u64) -> Option<(u8, bool, String)> {
    let active = sessions
        .iter()
        .filter(|s| {
            s.needs_attention()
                || match s.status {
                    Status::Waiting | Status::Working => true,
                    Status::Error => now.saturating_sub(s.signal_ms) < 60_000,
                    Status::Ready | Status::Interrupted => now.saturating_sub(s.signal_ms) < 12_000,
                    _ => false,
                }
        })
        .max_by_key(|s| {
            (
                if s.needs_attention() {
                    90
                } else {
                    match s.status {
                        Status::Waiting => 90,
                        Status::Error => 80,
                        Status::Working => 25,
                        _ => 55,
                    }
                },
                s.signal_ms,
            )
        })?;
    let priority = if active.needs_attention() {
        90
    } else {
        match active.status {
            Status::Waiting => 90,
            Status::Error => 80,
            Status::Working => 25,
            _ => 55,
        }
    };
    Some((
        priority,
        active.needs_attention() || matches!(active.status, Status::Working | Status::Waiting),
        format!(
            "{}:{}:{:?}:{}:{}",
            active.provider.key(),
            active.id,
            active.status,
            active.signal_ms,
            active.needs_attention()
        ),
    ))
}

pub fn clean(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(*ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(max)
        .collect()
}
