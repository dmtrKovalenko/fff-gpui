use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(alias = "launch_on_startup")]
    pub launch_at_login: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_keybind: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            launch_at_login: true,
            global_keybind: None,
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

pub fn watch_config_path(
    path: PathBuf,
    tx: async_channel::Sender<()>,
) -> Result<notify::RecommendedWatcher> {
    let watched_path = path.clone();
    let parent = config_parent(&path);
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                if event.paths.iter().any(|event_path| {
                    event_path == &watched_path
                        || parent.as_ref().is_some_and(|parent| event_path == parent)
                }) {
                    debug!(?event.kind, paths = ?event.paths, "config file event");
                    let _ = tx.send_blocking(());
                }
            }
            Err(err) => {
                warn!(error = %err, "config watcher error");
            }
        })?;

    if let Some(parent) = config_parent(&path)
        && parent.exists()
    {
        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .with_context(|| format!("failed to watch config path {}", parent.display()))?;
    }

    if path.exists() {
        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .with_context(|| format!("failed to watch config path {}", path.display()))?;
    }

    Ok(watcher)
}
