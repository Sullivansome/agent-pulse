//! Explicit, additive hook setup. Never modifies authentication or hook trust.
use crate::{hooks, model::Provider, store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MARKER: &str = "--island-agent-pulse";

pub fn hook_command(data: &Path, provider: Provider) -> String {
    format!(
        "{} hook --provider {} --data-dir {} {MARKER}",
        quote(&data.join("bin/island-agent-pulse").to_string_lossy()),
        provider.key(),
        quote(&data.to_string_lossy())
    )
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn config_path(home: &Path, provider: Provider, use_env: bool) -> PathBuf {
    let (env, folder, suffix) = match provider {
        Provider::Claude => ("CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
        Provider::Codex => ("CODEX_HOME", ".codex", "hooks.json"),
        Provider::Grok => ("GROK_HOME", ".grok", "hooks/island-agent-pulse.json"),
    };
    let base = if use_env {
        std::env::var_os(env).map(PathBuf::from)
    } else {
        None
    };
    base.unwrap_or_else(|| home.join(folder)).join(suffix)
}

pub fn merge(value: &mut Value, provider: Provider, command: Option<&str>) -> Result<()> {
    let obj = value
        .as_object_mut()
        .context("hook configuration must be a JSON object")?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be an object")?;
    for groups in hooks.values_mut() {
        let groups = groups
            .as_array_mut()
            .context("hook event groups must be arrays")?;
        for group in groups.iter_mut() {
            let handlers = group
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .context("hook handlers must be arrays")?;
            handlers.retain(|h| {
                !h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| c.contains(MARKER))
            });
        }
        groups.retain(|g| g["hooks"].as_array().is_some_and(|a| !a.is_empty()));
    }
    hooks.retain(|_, v| v.as_array().is_some_and(|a| !a.is_empty()));
    if let Some(command) = command {
        for event in hooks::events(provider) {
            let group = json!({"hooks": [{"type": "command", "command": command, "timeout": 2}]});
            hooks
                .entry(event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap()
                .push(group);
        }
    }
    Ok(())
}

pub fn read_config(path: &Path) -> Result<Value> {
    if path.is_symlink() {
        bail!("Refusing to replace symlinked config: {}", path.display());
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = vec![];
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        bail!("Hook config exceeds 2 MiB");
    }
    serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid JSON in {}; left unchanged", path.display()))
}

/// Validate every destination before changing any config; back up each changed file.
pub fn configure(
    home: &Path,
    data: &Path,
    binary: &Path,
    providers: &[Provider],
    remove: bool,
    use_env: bool,
) -> Result<Vec<PathBuf>> {
    if !cfg!(unix) {
        bail!("Automatic hook setup currently supports macOS and Linux");
    }
    let _lock = store::lock(data)?;
    let helper = data.join("bin/island-agent-pulse");
    let mut changes = vec![];
    for &provider in providers {
        let path = config_path(home, provider, use_env);
        let old = read_config(&path)?;
        let command = hook_command(data, provider);
        let mut new = old.clone();
        merge(
            &mut new,
            provider,
            if remove { None } else { Some(&command) },
        )?;
        if old != new {
            changes.push((path, new));
        }
    }
    if !remove {
        store::private_dir(helper.parent().unwrap())?;
        if binary != helper {
            // A stable helper survives package replacement and debug/release switches.
            let mut temp = tempfile::NamedTempFile::new_in(helper.parent().unwrap())?;
            std::io::copy(&mut fs::File::open(binary)?, &mut temp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                temp.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o700))?;
            }
            temp.persist(&helper)?;
        }
    }
    let mut paths = vec![];
    for (path, value) in changes {
        fs::create_dir_all(path.parent().unwrap())?;
        if path.exists() {
            let mut backup = tempfile::Builder::new()
                .prefix("island-agent-pulse-backup-")
                .suffix(".json")
                .tempfile_in(path.parent().unwrap())?;
            std::io::copy(&mut fs::File::open(&path)?, &mut backup)?;
            backup.keep()?;
        }
        store::atomic_write(&path, &serde_json::to_vec_pretty(&value)?)?;
        paths.push(path);
    }
    if remove {
        let _ = fs::remove_file(data.join("connected.json"));
    } else {
        store::atomic_write(
            &data.join("connected.json"),
            &serde_json::to_vec(providers)?,
        )?;
    }
    Ok(paths)
}
