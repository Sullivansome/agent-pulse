//! Read only process identity, ancestry and controlling-terminal metadata.
//! Never read command arguments, environment variables or terminal contents.
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProcess {
    pub pid: u32,
    /// Platform birth token prevents a recycled PID from keeping a row alive.
    pub started: String,
    pub terminal: u64,
}

pub(crate) struct Snapshot {
    pub parent: u32,
    pub name: String,
    pub started: String,
    pub terminal: Option<u64>,
    pub running: bool,
}

pub fn terminal_process(pid: u32) -> Option<TerminalProcess> {
    let process = inspect(pid).ok().flatten()?;
    process.running.then_some(TerminalProcess {
        pid,
        started: process.started,
        terminal: process.terminal?,
    })
}

/// Unknown/denied inspection is not evidence that a terminal was closed.
pub fn is_alive(expected: &TerminalProcess) -> Option<bool> {
    match inspect(expected.pid) {
        Ok(Some(current)) => Some(
            current.running
                && current.started == expected.started
                && current.terminal == Some(expected.terminal),
        ),
        Ok(None) => Some(false),
        Err(_) => None,
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn bsd_info(pid: u32) -> io::Result<Option<libc::proc_bsdinfo>> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 1)
        .ok_or_else(|| io::Error::other("invalid process id"))?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let len = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            len,
        )
    };
    if read == len {
        Ok(Some(unsafe { info.assume_init() }))
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn inspect(pid: u32) -> io::Result<Option<Snapshot>> {
    Ok(bsd_info(pid)?.map(|info| Snapshot {
        parent: info.pbi_ppid,
        name: info
            .pbi_comm
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8 as char)
            .collect(),
        started: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        terminal: (info.e_tdev != u32::MAX).then_some(u64::from(info.e_tdev)),
        running: info.pbi_status != libc::SZOMB,
    }))
}

/// The short summary is available across a privileged login helper; the full
/// BSD record is restricted to the same user. Only ancestry needs this access.
#[cfg(target_os = "macos")]
pub(crate) fn parent_pid(pid: u32) -> Option<u32> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 1)?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdshortinfo>::zeroed();
    let len = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            len,
        )
    };
    (read == len).then(|| unsafe { info.assume_init() }.pbsi_ppid)
}

#[cfg(any(target_os = "linux", test))]
fn linux_stat(text: &str, boot: &str) -> io::Result<Snapshot> {
    // comm can itself contain spaces and parentheses. Remaining fields start at state (3).
    let parse = || {
        let (head, tail) = text.rsplit_once(')')?;
        let name = head.split_once('(')?.1.to_owned();
        let fields: Vec<_> = tail.split_whitespace().collect();
        let terminal = fields.get(4)?.parse::<i64>().ok()?;
        let started = fields.get(19)?.parse::<u64>().ok()?;
        Some(Snapshot {
            parent: fields.get(1)?.parse().ok()?,
            name,
            started: format!("{boot}:{started}"),
            terminal: (terminal != 0).then_some(terminal as u32 as u64),
            running: !matches!(*fields.first()?, "Z" | "X" | "x"),
        })
    };
    parse().ok_or_else(|| io::Error::other("invalid process stat"))
}

#[cfg(target_os = "linux")]
pub(crate) fn inspect(pid: u32) -> io::Result<Option<Snapshot>> {
    use std::sync::OnceLock;
    static BOOT: OnceLock<Option<String>> = OnceLock::new();
    let boot = BOOT
        .get_or_init(|| {
            std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .ok()
                .map(|s| s.trim().into())
        })
        .as_ref()
        .ok_or_else(|| io::Error::other("boot identity unavailable"))?;
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => linux_stat(&text, boot).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn inspect(_: u32) -> io::Result<Option<Snapshot>> {
    Err(io::Error::other("process observation unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_identity_handles_names_and_boots_without_reading_arguments() {
        let text = "123 (agent (worker)) S 45 123 123 34817 123 0 0 0 0 0 0 0 0 0 20 0 1 0 5678";
        let snapshot = linux_stat(text, "boot-a").unwrap();
        assert_eq!(snapshot.name, "agent (worker)");
        assert_eq!(snapshot.parent, 45);
        assert_eq!(snapshot.terminal, Some(34817));
        assert_eq!(snapshot.started, "boot-a:5678");
        assert!(snapshot.running);
        assert_ne!(
            snapshot.started,
            linux_stat(text, "boot-b").unwrap().started
        );
        assert!(
            !linux_stat(&text.replace(") S ", ") Z "), "boot-a")
                .unwrap()
                .running
        );
        assert!(linux_stat("incomplete", "boot-a").is_err());
    }
}
