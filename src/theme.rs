use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    OnceLock, RwLock,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context as _, Result};
use gpui::{App, Global, SharedString, WindowAppearance};
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, warn};

use crate::config::{AppConfig, DEFAULT_PICKER_PANE_WIDTH};

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
pub const DEFAULT_UI_FONT_SIZE: f32 = 16.0;
pub const DEFAULT_BUFFER_FONT_SIZE: f32 = 15.0;

static ACTIVE_THEME: OnceLock<RwLock<AppTheme>> = OnceLock::new();
static THEME_VERSION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq)]
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
    pub match_highlight_bg: u32,
    pub preview_bg: u32,
    pub input_bg: u32,
    pub input_text: u32,
    pub cursor: u32,
    pub cursor_selection: u32,
    pub picker_pane_width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppTheme {
    pub bg: u32,
    pub border: u32,
    pub selected_row: u32,
    pub hover_row: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_dim: u32,
    pub status_bar_bg: u32,
    pub match_highlight: u32,
    pub match_highlight_bg: u32,
    pub preview_bg: u32,
    pub input_bg: u32,
    pub input_text: u32,
    pub cursor: u32,
    pub cursor_selection: u32,
    pub ui_font_family: Option<String>,
    pub buffer_font_family: Option<String>,
    pub ui_font_size: f32,
    pub buffer_font_size: f32,
    pub picker_pane_width: f32,
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
            match_highlight_bg: 0x2C4870,
            preview_bg: DEFAULT_PREVIEW_BG,
            input_bg: 0x232326,
            input_text: 0xE5E5EA,
            cursor: 0x0A84FF,
            cursor_selection: 0x0A84FF44,
            picker_pane_width: DEFAULT_PICKER_PANE_WIDTH,
        }
    }
}

impl Default for AppTheme {
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
            match_highlight_bg: 0x2C4870,
            preview_bg: DEFAULT_PREVIEW_BG,
            input_bg: 0x232326,
            input_text: 0xE5E5EA,
            cursor: 0x0A84FF,
            cursor_selection: 0x0A84FF44,
            ui_font_family: Some(DEFAULT_UI_FONT_FAMILY.to_string()),
            buffer_font_family: Some(DEFAULT_BUFFER_FONT_FAMILY.to_string()),
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            buffer_font_size: DEFAULT_BUFFER_FONT_SIZE,
            picker_pane_width: DEFAULT_PICKER_PANE_WIDTH,
            syntax_styles: Vec::new(),
            syntax_default_color: DEFAULT_TEXT_PRIMARY,
        }
    }
}

impl Global for AppTheme {}

impl AppTheme {
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
    #[serde(default)]
    ui_font_size: Option<f32>,
    #[serde(default)]
    buffer_font_size: Option<f32>,
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

const BUILTIN_THEME_FAMILIES: &[(&str, &str)] = &[
    (
        "vendor/zed/themes/ayu/ayu.json",
        include_str!("../vendor/zed/themes/ayu/ayu.json"),
    ),
    (
        "vendor/zed/themes/gruvbox/gruvbox.json",
        include_str!("../vendor/zed/themes/gruvbox/gruvbox.json"),
    ),
    (
        "vendor/zed/themes/one/one.json",
        include_str!("../vendor/zed/themes/one/one.json"),
    ),
];

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

fn active_theme_lock() -> &'static RwLock<AppTheme> {
    ACTIVE_THEME.get_or_init(|| RwLock::new(AppTheme::default()))
}

pub fn current() -> AppTheme {
    match active_theme_lock().read() {
        Ok(theme) => theme.clone(),
        Err(_) => AppTheme::default(),
    }
}

pub fn palette() -> Palette {
    let theme = current();
    Palette {
        bg: theme.bg,
        border: theme.border,
        selected_row: theme.selected_row,
        hover_row: theme.hover_row,
        text_primary: theme.text_primary,
        text_secondary: theme.text_secondary,
        text_dim: theme.text_dim,
        status_bar_bg: theme.status_bar_bg,
        match_highlight: theme.match_highlight,
        match_highlight_bg: theme.match_highlight_bg,
        preview_bg: theme.preview_bg,
        input_bg: theme.input_bg,
        input_text: theme.input_text,
        cursor: theme.cursor,
        cursor_selection: theme.cursor_selection,
        picker_pane_width: theme.picker_pane_width,
    }
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
    let need_theme_catalog = config.sync_zed_settings || config.theme.name.is_some();
    let theme_catalog = if need_theme_catalog {
        match load_theme_catalog() {
            Ok(catalog) => Some(catalog),
            Err(err) => {
                warn!(error = %err, "failed to load theme catalog; falling back to defaults");
                None
            }
        }
    } else {
        None
    };

    let mut resolved = if config.sync_zed_settings {
        match theme_catalog.as_ref() {
            Some(catalog) => match resolve_from_zed_settings(appearance, catalog) {
                Ok(theme) => theme,
                Err(err) => {
                    warn!(error = %err, "failed to sync Zed theme settings; falling back to defaults");
                    AppTheme::default()
                }
            },
            None => AppTheme::default(),
        }
    } else {
        AppTheme::default()
    };

    if let Some(catalog) = theme_catalog.as_ref()
        && let Some(name) = config.theme.name.as_deref()
    {
        if !name.trim().is_empty() {
            apply_named_theme(name, catalog, &mut resolved);
        }
    }

    if let Some(family) = resolve_optional_string(
        config.font.ui_family.as_deref(),
        config.font.family.as_deref(),
    ) {
        resolved.ui_font_family = Some(family);
    }
    if let Some(family) = resolve_optional_string(
        config.font.buffer_family.as_deref(),
        config.font.family.as_deref(),
    ) {
        resolved.buffer_font_family = Some(family);
    }
    if let Some(size) = resolve_optional_font_size(config.font.ui_size, config.font.size) {
        resolved.ui_font_size = size;
    }
    if let Some(size) = resolve_optional_font_size(config.font.buffer_size, config.font.size) {
        resolved.buffer_font_size = size;
    }
    if config.picker_pane_width.is_finite() && config.picker_pane_width > 0.0 {
        resolved.picker_pane_width = config.picker_pane_width;
    }
    apply_color(&config.theme.bg, &mut resolved.bg);
    apply_color(&config.theme.border, &mut resolved.border);
    apply_color(&config.theme.selected_row, &mut resolved.selected_row);
    apply_color(&config.theme.hover_row, &mut resolved.hover_row);
    apply_color(&config.theme.text_primary, &mut resolved.text_primary);
    apply_color(&config.theme.text_secondary, &mut resolved.text_secondary);
    apply_color(&config.theme.text_dim, &mut resolved.text_dim);
    apply_color(&config.theme.status_bar_bg, &mut resolved.status_bar_bg);
    apply_color(&config.theme.match_highlight, &mut resolved.match_highlight);
    apply_color(&config.theme.match_highlight_bg, &mut resolved.match_highlight_bg);
    apply_color(&config.theme.preview_bg, &mut resolved.preview_bg);
    apply_color(&config.theme.input_bg, &mut resolved.input_bg);
    apply_color(&config.theme.input_text, &mut resolved.input_text);
    apply_color(&config.theme.cursor, &mut resolved.cursor);
    apply_color(&config.theme.cursor_selection, &mut resolved.cursor_selection);

    cx.set_global(resolved.clone());
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

fn load_zed_settings() -> Result<ZedSettings> {
    let path = zed_settings_path();
    if !path.exists() {
        return Ok(ZedSettings::default());
    }

    let contents = read_to_string(&path)?;
    json5::from_str(&contents)
        .with_context(|| format!("failed to parse JSON from {}", path.display()))
}

fn resolve_from_zed_settings(
    appearance: WindowAppearance,
    catalog: &HashMap<String, ThemeCatalogEntry>,
) -> Result<AppTheme> {
    let settings = load_zed_settings()?;
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

    let mut theme = AppTheme::default();
    if let Some(name) = resolved_name {
        if let Some(entry) = catalog.get(&normalize_name(&name)) {
            apply_catalog_entry(entry, &mut theme);
        } else {
            warn!(theme = %name, "Zed theme not found; using built-in fallback theme");
        }
    } else {
        debug!(
            settings_path = %zed_settings_path().display(),
            "no Zed theme configured; using built-in fallback theme"
        );
    }

    theme.ui_font_family = Some(ui_font_family.to_string());
    theme.buffer_font_family = Some(buffer_font_family.to_string());
    theme.ui_font_size = settings.ui_font_size.unwrap_or(DEFAULT_UI_FONT_SIZE);
    theme.buffer_font_size = settings.buffer_font_size.unwrap_or(DEFAULT_BUFFER_FONT_SIZE);

    Ok(theme)
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
    load_builtin_theme_catalog(&mut catalog)?;
    load_installed_theme_catalog(&mut catalog)?;
    load_local_theme_catalog(&mut catalog)?;
    Ok(catalog)
}

fn load_builtin_theme_catalog(catalog: &mut HashMap<String, ThemeCatalogEntry>) -> Result<()> {
    for &(label, contents) in BUILTIN_THEME_FAMILIES {
        load_theme_family_contents(label, contents, catalog)?;
    }

    Ok(())
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
    let contents = read_to_string(path)?;
    load_theme_family_contents(&path.display().to_string(), &contents, catalog)
}

fn load_theme_family_contents(
    label: &str,
    contents: &str,
    catalog: &mut HashMap<String, ThemeCatalogEntry>,
) -> Result<()> {
    let family: ThemeFamilyFile = json5::from_str(contents)
        .with_context(|| format!("failed to parse JSON from {label}"))?;

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

fn apply_named_theme(name: &str, catalog: &HashMap<String, ThemeCatalogEntry>, theme: &mut AppTheme) {
    if let Some(entry) = catalog.get(&normalize_name(name)) {
        apply_catalog_entry(entry, theme);
    } else {
        warn!(theme = %name, "theme override not found in catalog");
    }
}

fn apply_catalog_entry(entry: &ThemeCatalogEntry, theme: &mut AppTheme) {
    theme.bg = entry.palette.bg;
    theme.border = entry.palette.border;
    theme.selected_row = entry.palette.selected_row;
    theme.hover_row = entry.palette.hover_row;
    theme.text_primary = entry.palette.text_primary;
    theme.text_secondary = entry.palette.text_secondary;
    theme.text_dim = entry.palette.text_dim;
    theme.status_bar_bg = entry.palette.status_bar_bg;
    theme.match_highlight = entry.palette.match_highlight;
    theme.match_highlight_bg = entry.palette.match_highlight_bg;
    theme.preview_bg = entry.palette.preview_bg;
    theme.input_bg = entry.palette.input_bg;
    theme.input_text = entry.palette.input_text;
    theme.cursor = entry.palette.cursor;
    theme.cursor_selection = entry.palette.cursor_selection;
    theme.syntax_styles = entry.syntax_styles.clone();
    theme.syntax_default_color = entry.syntax_default_color;
}

fn resolve_optional_string(primary: Option<&str>, fallback: Option<&str>) -> Option<String> {
    primary
        .and_then(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
        .or_else(|| {
            fallback.and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            })
        })
}

fn resolve_optional_font_size(primary: Option<f32>, fallback: Option<f32>) -> Option<f32> {
    primary
        .filter(|value| value.is_finite() && *value > 0.0)
        .or_else(|| fallback.filter(|value| value.is_finite() && *value > 0.0))
}

fn palette_from_style(style: &Value) -> Palette {
    Palette {
        bg: color_from_style(style, "background")
            .or_else(|| color_from_style(style, "surface.background"))
            .or_else(|| color_from_style(style, "editor.background"))
            .unwrap_or(DEFAULT_BG),
        border: color_from_style(style, "border").unwrap_or(DEFAULT_BORDER),
        selected_row: color_from_style(style, "ghost_element.selected")
            .or_else(|| color_from_style(style, "elevated_surface.background"))
            .or_else(|| color_from_style(style, "drop_target.background"))
            .or_else(|| color_from_style(style, "element.selected"))
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
        match_highlight_bg: color_from_style(style, "search.match_background")
            .or_else(|| color_from_style(style, "search.active_match_background"))
            .unwrap_or(0x2C4870),
        preview_bg: color_from_style(style, "editor.background")
            .or_else(|| color_from_style(style, "surface.background"))
            .unwrap_or(DEFAULT_PREVIEW_BG),
        input_bg: color_from_style(style, "input.background").unwrap_or(0x232326),
        input_text: color_from_style(style, "input.foreground").unwrap_or(0xE5E5EA),
        cursor: color_from_style(style, "editor.cursor").unwrap_or(0x0A84FF),
        cursor_selection: color_from_style(style, "editor.selectionBackground")
            .unwrap_or(0x0A84FF44),
        picker_pane_width: DEFAULT_PICKER_PANE_WIDTH,
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

fn apply_color(source: &Option<String>, target: &mut u32) {
    if let Some(color) = source.as_deref().and_then(parse_color_rgb) {
        *target = color;
    }
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
        let theme = AppTheme {
            syntax_styles: vec![
                ("variable".to_string(), syntax_style(0x112233)),
                ("constant".to_string(), syntax_style(0x445566)),
                ("constructor".to_string(), syntax_style(0x778899)),
                ("punctuation".to_string(), syntax_style(0xaabbcc)),
            ],
            syntax_default_color: 0xddeeff,
            ..AppTheme::default()
        };

        assert_eq!(theme.syntax_color("constant"), 0x112233);
        assert_eq!(theme.syntax_color("constructor"), 0x112233);
        assert_eq!(theme.syntax_color("type"), 0x112233);
        assert_eq!(theme.syntax_color("punctuation"), 0xddeeff);
        assert_eq!(theme.syntax_color("punctuation.bracket"), 0xddeeff);
    }

    #[test]
    fn builtin_theme_catalog_includes_zed_themes() {
        let catalog = load_theme_catalog().expect("theme catalog should load");

        assert!(catalog.contains_key("ayu dark"));
        assert!(catalog.contains_key("gruvbox dark"));
        assert!(catalog.contains_key("one dark"));
    }
}
