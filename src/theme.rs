use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    OnceLock, RwLock,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context as _, Result};
use gpui::{App, SharedString, WindowAppearance};
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, warn};

use crate::config::AppConfig;

const DEFAULT_BG: u32 = 0x1C1C1E;
const DEFAULT_BORDER: u32 = 0x2F2F31;
const DEFAULT_SELECTED_ROW: u32 = 0x2C3F59;
const DEFAULT_HOVER_ROW: u32 = 0x2A2A2C;
const DEFAULT_TEXT_PRIMARY: u32 = 0xFFFFFF;
const DEFAULT_TEXT_SECONDARY: u32 = 0x8E8E93;
const DEFAULT_TEXT_DIM: u32 = 0x6C6C70;
const DEFAULT_STATUS_BAR_BG: u32 = 0x18181A;
const DEFAULT_MATCH_HIGHLIGHT: u32 = 0x4A9EFF;
const DEFAULT_PREVIEW_BG: u32 = 0x161618;
const DEFAULT_UI_FONT_FAMILY: &str = ".ZedSans";
const DEFAULT_BUFFER_FONT_FAMILY: &str = ".ZedMono";

static ACTIVE_THEME: OnceLock<RwLock<ResolvedTheme>> = OnceLock::new();
static THEME_VERSION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub border: u32,
    pub selected_row: u32,
    pub hover_row: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_dim: u32,
    pub status_bar_bg: u32,
    pub match_highlight: u32,
    pub preview_bg: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTheme {
    pub palette: Palette,
    pub ui_font_family: SharedString,
    pub buffer_font_family: SharedString,
    pub syntax_styles: Vec<(String, SyntaxStyle)>,
    pub syntax_default_color: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyntaxRenderStyle {
    pub color: u32,
    pub italic: bool,
    pub bold: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: DEFAULT_BG,
            border: DEFAULT_BORDER,
            selected_row: DEFAULT_SELECTED_ROW,
            hover_row: DEFAULT_HOVER_ROW,
            text_primary: DEFAULT_TEXT_PRIMARY,
            text_secondary: DEFAULT_TEXT_SECONDARY,
            text_dim: DEFAULT_TEXT_DIM,
            status_bar_bg: DEFAULT_STATUS_BAR_BG,
            match_highlight: DEFAULT_MATCH_HIGHLIGHT,
            preview_bg: DEFAULT_PREVIEW_BG,
        }
    }
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            ui_font_family: DEFAULT_UI_FONT_FAMILY.into(),
            buffer_font_family: DEFAULT_BUFFER_FONT_FAMILY.into(),
            syntax_styles: Vec::new(),
            syntax_default_color: DEFAULT_TEXT_PRIMARY,
        }
    }
}

impl ResolvedTheme {
    fn syntax_color(&self, capture_name: &str) -> u32 {
        if syntax_capture_is_punctuation(capture_name) {
            return self.syntax_default_color;
        }

        if syntax_capture_uses_variable_color(capture_name) {
            return syntax_color_from_styles(&self.syntax_styles, "variable", self.syntax_default_color);
        }

        syntax_color_from_styles(
            &self.syntax_styles,
            capture_name,
            self.syntax_default_color,
        )
    }

    fn syntax_render_style(&self, capture_name: &str) -> SyntaxRenderStyle {
        if syntax_capture_is_punctuation(capture_name) {
            return SyntaxRenderStyle {
                color: self.syntax_default_color,
                ..Default::default()
            };
        }

        let resolved_name = if syntax_capture_uses_variable_color(capture_name) {
            "variable"
        } else {
            capture_name
        };

        let style = syntax_style_for_capture(&self.syntax_styles, resolved_name);
        SyntaxRenderStyle {
            color: syntax_style_color(&style).unwrap_or(self.syntax_default_color),
            italic: matches!(style.font_style.as_deref(), Some("italic")),
            bold: matches!(style.font_style.as_deref(), Some("bold"))
                || style.font_weight.is_some_and(|w| w >= 600.0),
            underline: matches!(style.font_style.as_deref(), Some("underline")),
            strikethrough: matches!(style.font_style.as_deref(), Some("strikethrough")),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
enum ThemeMode {
    Dark,
    Light,
    #[default]
    System,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ThemeSelection {
    Static(String),
    Dynamic {
        #[serde(default)]
        mode: ThemeMode,
        light: String,
        dark: String,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ZedSettings {
    #[serde(default)]
    theme: Option<ThemeSelection>,
    #[serde(default)]
    ui_font_family: Option<String>,
    #[serde(default)]
    buffer_font_family: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ThemeFamilyFile {
    #[serde(default)]
    themes: Vec<ThemeVariant>,
}

#[derive(Debug, Clone, Deserialize)]
struct ThemeVariant {
    name: String,
    #[serde(default)]
    style: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ExtensionManifest {
    #[serde(default)]
    themes: Vec<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct SyntaxStyle {
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    background_color: Option<String>,
    #[serde(default)]
    font_style: Option<String>,
    #[serde(default)]
    font_weight: Option<f32>,
}

#[derive(Debug, Clone)]
struct ThemeCatalogEntry {
    palette: Palette,
    syntax_styles: Vec<(String, SyntaxStyle)>,
    syntax_default_color: u32,
}

fn normalize_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn active_theme_lock() -> &'static RwLock<ResolvedTheme> {
    ACTIVE_THEME.get_or_init(|| RwLock::new(ResolvedTheme::default()))
}

pub fn current() -> ResolvedTheme {
    match active_theme_lock().read() {
        Ok(theme) => theme.clone(),
        Err(_) => ResolvedTheme::default(),
    }
}

pub fn palette() -> Palette {
    current().palette
}

pub fn ui_font_family() -> SharedString {
    active_theme_lock()
        .read()
        .map(|theme| theme.ui_font_family.clone())
        .unwrap_or_else(|_| DEFAULT_UI_FONT_FAMILY.into())
}

pub fn buffer_font_family() -> SharedString {
    active_theme_lock()
        .read()
        .map(|theme| theme.buffer_font_family.clone())
        .unwrap_or_else(|_| DEFAULT_BUFFER_FONT_FAMILY.into())
}

pub fn syntax_color(capture_name: &str) -> u32 {
    match active_theme_lock().read() {
        Ok(theme) => theme.syntax_color(capture_name),
        Err(_) => DEFAULT_TEXT_PRIMARY,
    }
}

pub fn syntax_render_style(capture_name: &str) -> SyntaxRenderStyle {
    match active_theme_lock().read() {
        Ok(theme) => theme.syntax_render_style(capture_name),
        Err(_) => SyntaxRenderStyle {
            color: DEFAULT_TEXT_PRIMARY,
            ..Default::default()
        },
    }
}

pub fn version() -> u64 {
    THEME_VERSION.load(Ordering::SeqCst)
}

pub fn sync_from_config(config: &AppConfig, appearance: WindowAppearance, cx: &mut App) {
    let resolved = if config.sync_zed_settings {
        match resolve_from_zed_settings(appearance) {
            Ok(theme) => theme,
            Err(err) => {
                warn!(error = %err, "failed to sync Zed theme settings; falling back to defaults");
                ResolvedTheme::default()
            }
        }
    } else {
        ResolvedTheme::default()
    };

    if let Ok(mut guard) = active_theme_lock().write() {
        *guard = resolved;
    }
    THEME_VERSION.fetch_add(1, Ordering::SeqCst);

    refresh_windows(cx);
}

fn refresh_windows(cx: &mut App) {
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, _| {
            window.refresh();
        });
    }
}

fn zed_config_dir() -> PathBuf {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(config_home).join("zed")
    } else {
        home_dir().join(".config/zed")
    }
}

fn zed_settings_path() -> PathBuf {
    zed_config_dir().join("settings.json")
}

fn zed_local_themes_dir() -> PathBuf {
    zed_config_dir().join("themes")
}

fn zed_installed_themes_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return home_dir().join("Library/Application Support/Zed/extensions/installed");
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(data_home).join("zed/extensions/installed");
        }

        home_dir().join(".local/share/zed/extensions/installed")
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let contents = read_to_string(path)?;
    json5::from_str(&contents)
        .with_context(|| format!("failed to parse JSON from {}", path.display()))
}

fn load_zed_settings() -> Result<ZedSettings> {
    let path = zed_settings_path();
    if !path.exists() {
        return Ok(ZedSettings::default());
    }

    read_json(&path)
}

fn resolve_from_zed_settings(appearance: WindowAppearance) -> Result<ResolvedTheme> {
    let settings = load_zed_settings()?;
    let catalog = load_theme_catalog()?;
    let ui_font_family = SharedString::from(
        settings
            .ui_font_family
            .unwrap_or_else(|| DEFAULT_UI_FONT_FAMILY.to_string()),
    );
    let buffer_font_family = SharedString::from(
        settings
            .buffer_font_family
            .unwrap_or_else(|| DEFAULT_BUFFER_FONT_FAMILY.to_string()),
    );
    let resolved_name = settings
        .theme
        .as_ref()
        .map(|theme| resolve_theme_name(theme, appearance));

    Ok(match resolved_name {
        Some(name) => match catalog.get(&normalize_name(&name)).cloned() {
            Some(entry) => {
                ResolvedTheme {
                    palette: entry.palette,
                    ui_font_family,
                    buffer_font_family,
                    syntax_default_color: entry.syntax_default_color,
                    syntax_styles: entry.syntax_styles,
                }
            }
            None => {
                warn!(theme = %name, "Zed theme not found; using built-in fallback theme");
                ResolvedTheme {
                    ui_font_family,
                    buffer_font_family,
                    ..ResolvedTheme::default()
                }
            }
        },
        None => {
            debug!(
                settings_path = %zed_settings_path().display(),
                "no Zed theme configured; using built-in fallback theme"
            );
            ResolvedTheme {
                ui_font_family,
                buffer_font_family,
                ..ResolvedTheme::default()
            }
        }
    })
}

fn resolve_theme_name(selection: &ThemeSelection, appearance: WindowAppearance) -> String {
    match selection {
        ThemeSelection::Static(name) => name.clone(),
        ThemeSelection::Dynamic { mode, light, dark } => match mode {
            ThemeMode::Light => light.clone(),
            ThemeMode::Dark => dark.clone(),
            ThemeMode::System => match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => dark.clone(),
                WindowAppearance::Light | WindowAppearance::VibrantLight => light.clone(),
            },
        },
    }
}

fn load_theme_catalog() -> Result<HashMap<String, ThemeCatalogEntry>> {
    let mut catalog = HashMap::new();
    load_installed_theme_catalog(&mut catalog)?;
    load_local_theme_catalog(&mut catalog)?;
    Ok(catalog)
}

fn load_local_theme_catalog(catalog: &mut HashMap<String, ThemeCatalogEntry>) -> Result<()> {
    let dir = zed_local_themes_dir();
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&dir)
        .with_context(|| format!("reading local Zed themes from {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        load_theme_family_file(&path, catalog)?;
    }

    Ok(())
}

fn load_installed_theme_catalog(catalog: &mut HashMap<String, ThemeCatalogEntry>) -> Result<()> {
    let dir = zed_installed_themes_dir();
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&dir)
        .with_context(|| format!("reading installed Zed themes from {}", dir.display()))?
    {
        let entry = entry?;
        let extension_dir = entry.path();
        if !extension_dir.is_dir() {
            continue;
        }

        let manifest_path = extension_dir.join("extension.toml");
        if !manifest_path.exists() {
            continue;
        }

        let manifest: ExtensionManifest = match read_toml(&manifest_path) {
            Ok(manifest) => manifest,
            Err(err) => {
                warn!(error = %err, path = %manifest_path.display(), "skipping theme extension");
                continue;
            }
        };

        for relative_theme_path in manifest.themes {
            let theme_path = extension_dir.join(relative_theme_path);
            if let Err(err) = load_theme_family_file(&theme_path, catalog) {
                warn!(error = %err, path = %theme_path.display(), "failed to load installed Zed theme");
            }
        }
    }

    Ok(())
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let contents = read_to_string(path)?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse TOML from {}", path.display()))
}

fn load_theme_family_file(
    path: &Path,
    catalog: &mut HashMap<String, ThemeCatalogEntry>,
) -> Result<()> {
    let family: ThemeFamilyFile = read_json(path)?;

    for variant in family.themes {
        let theme_key = normalize_name(&variant.name);
        let entry = ThemeCatalogEntry {
            palette: palette_from_style(&variant.style),
            syntax_styles: syntax_styles_from_style(&variant.style),
            syntax_default_color: color_from_style(&variant.style, "editor.foreground")
                .or_else(|| color_from_style(&variant.style, "text"))
                .unwrap_or(DEFAULT_TEXT_PRIMARY),
        };

        catalog.insert(theme_key, entry);
    }

    Ok(())
}

fn palette_from_style(style: &Value) -> Palette {
    Palette {
        bg: color_from_style(style, "background")
            .or_else(|| color_from_style(style, "surface.background"))
            .or_else(|| color_from_style(style, "editor.background"))
            .unwrap_or(DEFAULT_BG),
        border: color_from_style(style, "border").unwrap_or(DEFAULT_BORDER),
        selected_row: color_from_style(style, "element.selected")
            .or_else(|| color_from_style(style, "element.active"))
            .unwrap_or(DEFAULT_SELECTED_ROW),
        hover_row: color_from_style(style, "element.hover").unwrap_or(DEFAULT_HOVER_ROW),
        text_primary: color_from_style(style, "text").unwrap_or(DEFAULT_TEXT_PRIMARY),
        text_secondary: color_from_style(style, "text.muted")
            .or_else(|| color_from_style(style, "icon.muted"))
            .unwrap_or(DEFAULT_TEXT_SECONDARY),
        text_dim: color_from_style(style, "text.placeholder")
            .or_else(|| color_from_style(style, "text.disabled"))
            .or_else(|| color_from_style(style, "icon.placeholder"))
            .unwrap_or(DEFAULT_TEXT_DIM),
        status_bar_bg: color_from_style(style, "status_bar.background")
            .or_else(|| color_from_style(style, "title_bar.background"))
            .unwrap_or(DEFAULT_STATUS_BAR_BG),
        match_highlight: color_from_style(style, "search.match_background")
            .or_else(|| color_from_style(style, "search.active_match_background"))
            .or_else(|| color_from_style(style, "text.accent"))
            .unwrap_or(DEFAULT_MATCH_HIGHLIGHT),
        preview_bg: color_from_style(style, "editor.background")
            .or_else(|| color_from_style(style, "surface.background"))
            .unwrap_or(DEFAULT_PREVIEW_BG),
    }
}

fn syntax_styles_from_style(style: &Value) -> Vec<(String, SyntaxStyle)> {
    let Some(syntax) = style.get("syntax").and_then(Value::as_object) else {
        return Vec::new();
    };

    syntax
        .iter()
        .filter_map(|(name, style_value)| {
            if name == "background_color" {
                return None;
            }

            let style_value = style_value.as_object()?;
            Some((name.clone(), SyntaxStyle::from_value(style_value)))
        })
        .collect()
}

fn syntax_color_from_styles(
    styles: &[(String, SyntaxStyle)],
    capture_name: &str,
    default_color: u32,
) -> u32 {
    let mut best_match: Option<(usize, usize, u32)> = None;

    for (index, (token, style)) in styles.iter().enumerate() {
        let mut specificity = 0;
        if syntax_token_matches_capture(token, capture_name, &mut specificity) {
            let candidate = (specificity, index, syntax_style_color(style).unwrap_or(default_color));
            if best_match.as_ref().is_none_or(|best| candidate > *best) {
                best_match = Some(candidate);
            }
        }
    }

    best_match.map_or(default_color, |(_, _, color)| color)
}

fn syntax_style_for_capture(styles: &[(String, SyntaxStyle)], capture_name: &str) -> SyntaxStyle {
    let mut best_match: Option<(usize, usize, SyntaxStyle)> = None;

    for (index, (token, style)) in styles.iter().enumerate() {
        let mut specificity = 0;
        if syntax_token_matches_capture(token, capture_name, &mut specificity) {
            let candidate = (specificity, index, style.clone());
            if best_match.as_ref().is_none_or(|best| candidate.0 > best.0 || (candidate.0 == best.0 && candidate.1 > best.1)) {
                best_match = Some(candidate);
            }
        }
    }

    best_match.map_or_else(SyntaxStyle::default, |(_, _, style)| style)
}

fn syntax_style_color(style: &SyntaxStyle) -> Option<u32> {
    style.color.as_deref().and_then(parse_color_rgb)
}

fn syntax_token_matches_capture(
    token: &str,
    capture_name: &str,
    specificity: &mut usize,
) -> bool {
    let capture_parts: Vec<&str> = capture_name.split('.').collect();
    let mut matched_parts = 0;

    for token_part in token.split('.') {
        if capture_parts.iter().any(|capture_part| capture_part == &token_part) {
            matched_parts += 1;
        } else {
            return false;
        }
    }

    *specificity = matched_parts;
    true
}

fn syntax_capture_is_punctuation(capture_name: &str) -> bool {
    matches!(capture_name, "punctuation" | "operator")
        || capture_name.starts_with("punctuation.")
}

fn syntax_capture_uses_variable_color(capture_name: &str) -> bool {
    matches!(capture_name, "constant" | "constructor" | "type")
        || capture_name.starts_with("constant.")
        || capture_name.starts_with("constructor.")
}

impl SyntaxStyle {
    fn from_value(value: &serde_json::Map<String, Value>) -> Self {
        let color = value
            .get("color")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let background_color = value
            .get("background_color")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let font_style = value
            .get("font_style")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let font_weight = value
            .get("font_weight")
            .and_then(Value::as_f64)
            .map(|w| w as f32);

        Self {
            color,
            background_color,
            font_style,
            font_weight,
        }
    }
}

fn color_from_style(style: &Value, key: &str) -> Option<u32> {
    style
        .get(key)
        .and_then(Value::as_str)
        .and_then(parse_color_rgb)
}

fn parse_color_rgb(color: &str) -> Option<u32> {
    let color = color.trim();
    let color = color.strip_prefix('#').unwrap_or(color);

    match color.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for ch in color.chars().take(3) {
                expanded.push(ch);
                expanded.push(ch);
            }
            u32::from_str_radix(&expanded, 16).ok()
        }
        4 => {
            let mut expanded = String::with_capacity(6);
            for ch in color.chars().take(3) {
                expanded.push(ch);
                expanded.push(ch);
            }
            u32::from_str_radix(&expanded, 16).ok()
        }
        6 => u32::from_str_radix(color, 16).ok(),
        8 => u32::from_str_radix(&color[..6], 16).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syntax_style(color: u32) -> SyntaxStyle {
        SyntaxStyle {
            color: Some(format!("#{color:06x}")),
            ..SyntaxStyle::default()
        }
    }

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_color_rgb("#ff00aa"), Some(0xff00aa));
        assert_eq!(parse_color_rgb("#ff00aaff"), Some(0xff00aa));
        assert_eq!(parse_color_rgb("#f0a"), Some(0xff00aa));
    }

    #[test]
    fn resolves_dynamic_theme_names() {
        let selection = ThemeSelection::Dynamic {
            mode: ThemeMode::System,
            light: "Light Theme".to_string(),
            dark: "Dark Theme".to_string(),
        };

        assert_eq!(
            resolve_theme_name(&selection, WindowAppearance::Dark),
            "Dark Theme"
        );
        assert_eq!(
            resolve_theme_name(&selection, WindowAppearance::Light),
            "Light Theme"
        );
    }

    #[test]
    fn syntax_color_prefers_later_matches_on_ties() {
        let styles = vec![
            ("foo.bar".to_string(), syntax_style(0x111111)),
            ("baz.qux".to_string(), syntax_style(0x222222)),
        ];

        assert_eq!(
            syntax_color_from_styles(&styles, "foo.bar.baz.qux", 0x999999),
            0x222222
        );
    }

    #[test]
    fn constant_and_punctuation_captures_follow_variable_and_text_colors() {
        let theme = ResolvedTheme {
            syntax_styles: vec![
                ("variable".to_string(), syntax_style(0x112233)),
                ("constant".to_string(), syntax_style(0x445566)),
                ("constructor".to_string(), syntax_style(0x778899)),
                ("punctuation".to_string(), syntax_style(0xaabbcc)),
            ],
            syntax_default_color: 0xddeeff,
            ..ResolvedTheme::default()
        };

        assert_eq!(theme.syntax_color("constant"), 0x112233);
        assert_eq!(theme.syntax_color("constructor"), 0x112233);
        assert_eq!(theme.syntax_color("type"), 0x112233);
        assert_eq!(theme.syntax_color("punctuation"), 0xddeeff);
        assert_eq!(theme.syntax_color("punctuation.bracket"), 0xddeeff);
    }
}
