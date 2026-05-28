use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use crate::paths::{default_game_root, expand_path};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub language: Option<String>,
    pub cdda_path: String,
    pub game_root: String,
    pub active_build: String,
    pub release_channel: String,
    pub steam_shortcut_name: String,
    pub use_steam_deck_konsole: bool,
    pub build_channels: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BuildChannelsConfig {
    Map(HashMap<String, String>),
    LegacyString(String),
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ConfigFile {
    language: Option<String>,
    cdda_path: String,
    game_root: String,
    active_build: String,
    release_channel: String,
    steam_shortcut_name: String,
    use_steam_deck_konsole: bool,
    build_channels: Option<BuildChannelsConfig>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        let config = Config::default();
        Self {
            language: config.language,
            cdda_path: config.cdda_path,
            game_root: config.game_root,
            active_build: config.active_build,
            release_channel: config.release_channel,
            steam_shortcut_name: config.steam_shortcut_name,
            use_steam_deck_konsole: config.use_steam_deck_konsole,
            build_channels: Some(BuildChannelsConfig::Map(config.build_channels)),
        }
    }
}

impl From<ConfigFile> for Config {
    fn from(file: ConfigFile) -> Self {
        let build_channels = match file.build_channels {
            Some(BuildChannelsConfig::Map(map)) => map,
            Some(BuildChannelsConfig::LegacyString(value)) => parse_build_channels(&value),
            None => HashMap::new(),
        };

        Self {
            language: normalize_language(file.language),
            cdda_path: file.cdda_path,
            game_root: normalize_game_root_value(&file.game_root),
            active_build: file.active_build,
            release_channel: file.release_channel,
            steam_shortcut_name: file.steam_shortcut_name,
            use_steam_deck_konsole: file.use_steam_deck_konsole,
            build_channels,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: None,
            cdda_path: String::from("~/Games/CDDA"),
            game_root: default_game_root().to_string_lossy().into_owned(),
            active_build: String::from(""),
            release_channel: String::from("experimental"),
            steam_shortcut_name: String::from("Cataclysm: Dark Days Ahead"),
            use_steam_deck_konsole: true,
            build_channels: HashMap::new(),
        }
    }
}

impl Config {
    pub fn game_root_path(&self) -> std::path::PathBuf {
        expand_path(&self.game_root)
    }

    pub fn channel_for_build(&self, build_id: &str) -> String {
        self.build_channels
            .get(build_id)
            .cloned()
            .unwrap_or_else(|| infer_channel_from_build_id(build_id))
    }

    pub fn register_build_channel(&mut self, build_id: &str, channel: &str) {
        self.build_channels
            .insert(build_id.to_string(), channel.to_string());
    }

    pub fn load(path: &Path) -> Self {
        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };

        toml::from_str::<ConfigFile>(&content)
            .map(Config::from)
            .unwrap_or_else(|_| load_legacy_config(&content))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;

        fs::write(path, content)
    }
}

fn normalize_language(language: Option<String>) -> Option<String> {
    match language.as_deref() {
        None | Some("system") | Some("") => None,
        _ => language,
    }
}

fn load_legacy_config(content: &str) -> Config {
    let mut config = Config::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = parse_config_value(value);

        match key {
            "language" => config.language = normalize_language(Some(value)),
            "cdda_path" => config.cdda_path = value,
            "game_root" => config.game_root = normalize_game_root_value(&value),
            "active_build" => config.active_build = value,
            "release_channel" => config.release_channel = value,
            "steam_shortcut_name" => config.steam_shortcut_name = value,
            "use_steam_deck_konsole" => {
                config.use_steam_deck_konsole = matches!(value.as_str(), "true" | "1" | "yes")
            }
            "build_channels" => config.build_channels = parse_build_channels(&value),
            _ => {}
        }
    }

    config
}

fn normalize_game_root_value(value: &str) -> String {
    let path = PathBuf::from(value);
    if path.file_name().and_then(|name| name.to_str()) == Some("gfx")
        && path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("cddock")
    {
        return path
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_else(|| value.to_string());
    }

    let normalized = value.replace('\\', "/");
    if let Some(root) = normalized.strip_suffix("/gfx")
        && root.ends_with("/cddock")
    {
        return root.to_string();
    }

    value.to_string()
}

pub fn infer_channel_from_build_id(build_id: &str) -> String {
    if build_id.contains("experimental") || build_id.starts_with("cdda-experimental") {
        String::from("experimental")
    } else {
        String::from("stable")
    }
}

fn parse_build_channels(value: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in value.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((build, channel)) = pair.split_once('=') else {
            continue;
        };
        map.insert(build.trim().to_string(), channel.trim().to_string());
    }
    map
}

pub fn config_path() -> std::path::PathBuf {
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return std::path::PathBuf::from(appdata)
                .join("cddock")
                .join("config.toml");
        }
    }

    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(xdg_config_home)
            .join("cddock")
            .join("config.toml");
    }

    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home)
            .join(".config")
            .join("cddock")
            .join("config.toml");
    }

    std::path::PathBuf::from("cddock.toml")
}

fn parse_config_value(value: &str) -> String {
    let value = value.trim();
    let value = value.split('#').next().unwrap_or(value).trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
}
