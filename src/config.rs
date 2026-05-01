use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

pub const DEFAULT_WINDOW_WIDTH: f32 = 960.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 520.0;

fn default_window_width() -> f32 {
    DEFAULT_WINDOW_WIDTH
}

fn default_window_height() -> f32 {
    DEFAULT_WINDOW_HEIGHT
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(alias = "launch_on_startup")]
    pub launch_at_login: bool,
    #[serde(rename = "sync-zed-settings")]
    pub sync_zed_settings: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_keybind: Option<String>,
    #[serde(default = "default_window_width")]
    pub window_width: f32,
    #[serde(default = "default_window_height")]
    pub window_height: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            launch_at_login: true,
            sync_zed_settings: true,
            global_keybind: None,
            window_width: DEFAULT_WINDOW_WIDTH,
            window_height: DEFAULT_WINDOW_HEIGHT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub path: PathBuf,
    pub config: AppConfig,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn preferred_config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("fff-gpui/config.toml")
    } else {
        home_dir().join(".config/fff-gpui/config.toml")
    }
}

fn legacy_config_path() -> PathBuf {
    home_dir().join(".fff-gpui.toml")
}

fn config_parent(path: &Path) -> Option<PathBuf> {
    path.parent().map(PathBuf::from)
}

pub fn active_config_path() -> PathBuf {
    let preferred = preferred_config_path();
    if preferred.exists() {
        return preferred;
    }

    let legacy = legacy_config_path();
    if legacy.exists() {
        return legacy;
    }

    preferred
}

pub fn load_active_config() -> Result<LoadedConfig> {
    let preferred = preferred_config_path();
    let legacy = legacy_config_path();

    let active = if preferred.exists() {
        preferred
    } else if legacy.exists() {
        legacy
    } else {
        preferred
    };

    if !active.exists() {
        ensure_config_file(&active, &AppConfig::default())?;
    }

    load_config_from(&active)
        .with_context(|| format!("failed to load config from {}", active.display()))
}

pub fn load_config_from(path: &Path) -> Result<LoadedConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let mut config = toml::from_str::<AppConfig>(&contents)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
    if config
        .global_keybind
        .as_deref()
        .is_some_and(|binding| binding.trim().is_empty())
    {
        config.global_keybind = None;
    }
    if !config.window_width.is_finite() || config.window_width <= 0.0 {
        warn!(
            value = config.window_width,
            "invalid window_width in config; falling back to default"
        );
        config.window_width = DEFAULT_WINDOW_WIDTH;
    }
    if !config.window_height.is_finite() || config.window_height <= 0.0 {
        warn!(
            value = config.window_height,
            "invalid window_height in config; falling back to default"
        );
        config.window_height = DEFAULT_WINDOW_HEIGHT;
    }
    Ok(LoadedConfig {
        path: path.to_path_buf(),
        config,
    })
}

pub fn ensure_config_file(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = config_parent(path) {
        fs::create_dir_all(&parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    if path.exists() {
        return Ok(());
    }

    write_config(path, config)?;
    Ok(())
}

pub fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = config_parent(path) {
        fs::create_dir_all(&parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(path, contents)
        .with_context(|| format!("failed to write config file {}", path.display()))?;
    info!(path = %path.display(), "wrote default config");
    Ok(())
}
