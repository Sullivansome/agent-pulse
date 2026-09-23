use crate::{
    model::{Event, RETAIN_MS, Sessions},
    now_ms,
};
use anyhow::{Context, Result, bail};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

// 128 bounded sessions, including up to 16 pending tool requests per session.
const MAX_BYTES: u64 = 1024 * 1024;

pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("state directory is a symlink");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn load(dir: &Path) -> Result<Sessions> {
    let file = match File::open(dir.join("sessions.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Sessions::default()),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = vec![];
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        bail!("session store exceeds size limit");
    }
    serde_json::from_slice(&bytes).context("read Agent Pulse state")
}

pub fn lock(dir: &Path) -> Result<File> {
    private_dir(dir)?;
    let path = dir.join("state.lock");
    if path.is_symlink() {
        bail!("state lock is a symlink");
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock)
                if started.elapsed() < Duration::from_millis(200) =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(anyhow::anyhow!("Agent Pulse state busy: {e}")),
        }
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("missing parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.persist(path)?;
    Ok(())
}

pub fn record(dir: &Path, event: Event) -> Result<()> {
    let _lock = lock(dir)?;
    let mut sessions = load(dir)?;
    sessions
        .sessions
        .retain(|s| now_ms().saturating_sub(s.updated_ms) < RETAIN_MS);
    sessions.apply(event);
    atomic_write(&dir.join("sessions.json"), &serde_json::to_vec(&sessions)?)
}
