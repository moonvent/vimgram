use directories::ProjectDirs;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(default = "default_theme")]
    pub theme: String,
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
