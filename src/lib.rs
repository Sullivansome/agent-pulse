pub mod connections;
pub mod grok;
pub mod hooks;
pub mod list;
pub mod model;
pub mod navigation;
pub mod process;
pub mod render;
pub mod setup;
pub mod store;
pub mod terminal;

pub const PLUGIN_ID: &str = "com.sullivansome.agent-pulse";

pub fn default_data_dir() -> std::path::PathBuf {
    island_plugin_api::plugins_dir()
        .join(PLUGIN_ID)
        .join("data")
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(target_os = "macos")]
mod native_text;
