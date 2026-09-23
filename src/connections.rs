//! Installation and observed activity are independent facts, never inferred trust.
use crate::{
    hooks,
    model::{Provider, STALE_MS, Session},
    setup,
};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Installation {
    #[default]
    NotInstalled,
    Installed,
    Incomplete,
    Unavailable,
}

impl Installation {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "No hooks installed",
            Self::Installed => "Hooks installed",
            Self::Incomplete => "Setup incomplete",
            Self::Unavailable => "Can't check setup",
        }
    }
}

#[derive(Default)]
pub struct Connections {
    pub installation: [Installation; 3],
    pub last_signal_ms: [Option<u64>; 3],
    pub activity_unavailable: bool,
}

impl Connections {
    pub fn configured(&self) -> bool {
        self.installation.contains(&Installation::Installed)
    }

    pub fn has_activity(&self) -> bool {
        self.last_signal_ms.iter().any(Option::is_some)
    }

    pub fn update_activity(&mut self, sessions: Option<&[Session]>) {
        self.activity_unavailable = sessions.is_none();
        self.last_signal_ms = Provider::ALL.map(|provider| {
            sessions.and_then(|sessions| {
                sessions
                    .iter()
                    .filter(|s| s.provider == provider)
                    .map(|s| s.updated_ms)
                    .max()
            })
        });
    }

    pub fn recent(&self, index: usize, now: u64) -> bool {
        !self.activity_unavailable
            && self.last_signal_ms[index].is_some_and(|last| now.saturating_sub(last) < STALE_MS)
    }

    pub fn activity_label(&self, index: usize, now: u64) -> String {
        if self.activity_unavailable {
            return "Can't read activity".into();
        }
        let Some(last) = self.last_signal_ms[index] else {
            return "No recorded activity".into();
        };
        let age = now.saturating_sub(last) / 1000;
        match age {
            0..60 => "Last signal just now".into(),
            60..3600 => format!("Last signal {}m ago", age / 60),
            3600..86400 => format!("Last signal {}h ago", age / 3600),
            _ => format!("Last signal {}d ago", age / 86400),
        }
    }

    pub fn action_label(&self) -> &'static str {
        if self.installation.contains(&Installation::Incomplete) {
            "Repair setup"
        } else if self.installation.contains(&Installation::NotInstalled) {
            "Install hooks"
        } else {
            "Reinstall hooks"
        }
    }

    pub fn hint(&self, now: u64) -> [&'static str; 2] {
        if self.activity_unavailable {
            [
                "Activity couldn't be read.",
                "This doesn't mean your agents disconnected.",
            ]
        } else if self.installation.contains(&Installation::Unavailable) {
            [
                "Some hook settings couldn't be checked.",
                "Your existing configuration hasn't been changed.",
            ]
        } else if self.installation.contains(&Installation::Incomplete) {
            [
                "Some hook files are missing or out of date.",
                "Repair setup to restore those hooks.",
            ]
        } else if (0..3).any(|i| self.recent(i, now)) {
            [
                "A recent signal confirms activity.",
                "Quiet agents may simply be idle.",
            ]
        } else if self.configured() {
            [
                "Hooks are installed; waiting for activity.",
                "Start a new task to check the connection.",
            ]
        } else {
            [
                "Install hooks to receive local agent status.",
                "Prompts and transcripts stay with your agents.",
            ]
        }
    }
}

/// Read the actual destinations and receiver, not a stale setup-success marker.
pub fn inspect(home: &Path, data: &Path, use_env: bool) -> [Installation; 3] {
    let receiver = data.join("bin/island-agent-pulse");
    let receiver_ready = receiver.metadata().is_ok_and(|meta| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.is_file() && meta.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        meta.is_file()
    });
    Provider::ALL.map(|provider| {
        let Ok(config) = setup::read_config(&setup::config_path(home, provider, use_env)) else {
            return Installation::Unavailable;
        };
        if !config.is_object()
            || config.get("hooks").is_some_and(|hooks| {
                hooks.as_object().is_none_or(|events| {
                    events.values().any(|groups| {
                        groups.as_array().is_none_or(|groups| {
                            groups.iter().any(|group| {
                                group
                                    .get("hooks")
                                    .is_none_or(|handlers| !handlers.is_array())
                            })
                        })
                    })
                })
            })
        {
            return Installation::Unavailable;
        }
        let expected = setup::hook_command(data, provider);
        let events = hooks::events(provider);
        let matches = events
            .iter()
            .filter(|event| {
                config["hooks"][**event].as_array().is_some_and(|groups| {
                    groups.iter().any(|group| {
                        group["hooks"].as_array().is_some_and(|handlers| {
                            handlers.iter().any(|handler| {
                                handler["type"] == "command"
                                    && handler["command"].as_str() == Some(&expected)
                            })
                        })
                    })
                })
            })
            .count();
        if matches == 0 {
            Installation::NotInstalled
        } else if matches == events.len() && receiver_ready {
            Installation::Installed
        } else {
            Installation::Incomplete
        }
    })
}
