use directories::ProjectDirs;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "Tokyo Night".into()
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
