use directories::ProjectDirs;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Folder to start in, by name (e.g. "Личка"); the main list otherwise.
    #[serde(default)]
    pub folder: Option<String>,
    /// Show an icon in the tray (waybar's `tray` module).
    #[serde(default = "yes")]
    pub tray: bool,
    /// Closing the window keeps vimgram running in the tray.
    #[serde(default = "yes")]
    pub close_to_tray: bool,
    /// Media of incoming messages in private chats and groups up to this
    /// size (MB) is downloaded right away, so it is kept if deleted.
    #[serde(default = "default_prefetch_mb")]
    pub prefetch_mb: u32,
    /// Moving through the chat list opens the selected chat beside it.
    #[serde(default)]
    pub open_on_select: bool,
    /// Where photos and videos open: over the whole window, or beside the
    /// chat (`s` in the viewer switches).
    #[serde(default)]
    pub viewer: ViewerMode,
    /// Times as 9:05 PM (`"12h"`) or 21:05 (`"24h"`).
    #[serde(default)]
    pub clock: Clock,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum Clock {
    #[default]
    #[serde(rename = "24h")]
    H24,
    #[serde(rename = "12h")]
    H12,
}

/// 12-hour clock, set once from the config at startup.
static CLOCK_12H: AtomicBool = AtomicBool::new(false);

pub fn set_clock(clock: Clock) {
    CLOCK_12H.store(clock == Clock::H12, Ordering::Relaxed);
}

/// `strftime` format of hours and minutes on the configured clock.
pub fn hm() -> &'static str {
    if CLOCK_12H.load(Ordering::Relaxed) { "%-I:%M %p" } else { "%H:%M" }
}

/// Hours, minutes and seconds on the configured clock.
pub fn hms() -> &'static str {
    if CLOCK_12H.load(Ordering::Relaxed) { "%-I:%M:%S %p" } else { "%H:%M:%S" }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewerMode {
    #[default]
    Full,
    Side,
}

fn default_theme() -> String {
    "Tokyo Night".into()
}

fn yes() -> bool {
    true
}

fn default_prefetch_mb() -> u32 {
    50
}

pub fn dirs() -> ProjectDirs {
    ProjectDirs::from("", "", "vimgram").expect("could not determine home directory")
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = dirs().config_dir().join("config.toml");
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_from_config() {
        let parse = |extra: &str| toml::from_str::<Config>(&format!("api_id = 1\napi_hash = \"x\"\n{extra}")).unwrap().clock;
        assert_eq!(parse(""), Clock::H24);
        assert_eq!(parse("clock = \"12h\""), Clock::H12);
        assert_eq!(parse("clock = \"24h\""), Clock::H24);
    }
}
