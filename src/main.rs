#![allow(unexpected_cfgs)]

mod assets;
mod config;
mod editor;
mod hotkey;
mod log;
mod menubar;
mod path_shortening;
mod picker;
mod preview;
mod service;
mod text_field;
mod theme;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use assets::{Assets, FontAssets};
use global_hotkey::GlobalHotKeyManager;
use gpui::prelude::*;
use gpui::*;
use tracing::{debug, info};

use config::AppConfig;
use picker::{
    CyclePreviousQuery, FffPicker, OpenSelected, PickerSharedState, PreviewScrollDown,
    PreviewScrollUp, Quit, SelectNext, SelectPrev, ShiftTab, SwitchFiles, SwitchGrep,
    ToggleSelectAll, ToggleSelectedAndAdvance,
};
use service::{
    CommandEnvelope, ForwardOutcome, ServiceCommand, forward_to_running_instance, start_listener,
};
use text_field::{
    FieldBackspace, FieldCopy, FieldCut, FieldDelete, FieldEnd, FieldHome, FieldLeft, FieldPaste,
    FieldRight, FieldSelectAll, FieldSelectLeft, FieldSelectRight,
};

actions!(fff_gpui, [ToggleWindow, OpenConfig]);

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Debug, Clone)]
struct LaunchOptions {
    base_path: PathBuf,
    open_path: Option<PathBuf>,
    base_path_explicit: bool,
    start_in_grep: bool,
    show_help: bool,
    show_version: bool,
}

type ResponderArc = Arc<Mutex<Option<service::ClientStream>>>;

#[derive(Clone)]
struct PickerSession {
    base_path: PathBuf,
    shared: PickerSharedState,
    enable_content_indexing: bool,
    start_in_grep: bool,
    responder: Option<ResponderArc>,
}

struct RuntimeConfig {
    config: AppConfig,
    config_path: PathBuf,
    hotkey_manager: Option<GlobalHotKeyManager>,
    registered_hotkey: Option<global_hotkey::hotkey::HotKey>,
}

impl PickerSession {
    fn new(base_path: PathBuf, enable_content_indexing: bool, start_in_grep: bool) -> Self {
        Self {
            base_path,
            shared: PickerSharedState::default(),
            enable_content_indexing,
            start_in_grep,
            responder: None,
        }
    }
}

// Resolve the user's home directory on Unix.
#[cfg(unix)]
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn normalize_dir(path: PathBuf) -> Option<PathBuf> {
    path.is_dir()
        .then(|| std::fs::canonicalize(&path).unwrap_or(path))
}

// Resolve the base directory from argv[1] or the user's home directory.
fn parse_launch_options() -> LaunchOptions {
    let mut base_path = None;
    let mut open_path = None;
    let mut start_in_grep = false;
    let mut show_help = false;
    let mut show_version = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        if arg == "-h" || arg == "--help" {
            show_help = true;
            continue;
        }

        if arg == "-V" || arg == "--version" {
            show_version = true;
            continue;
        }

        if arg == "--grep" {
            start_in_grep = true;
            continue;
        }

        if arg == "--open" {
            if let Some(path) = args.next().map(PathBuf::from).and_then(normalize_dir) {
                open_path = Some(path);
            }
            continue;
        }

        if base_path.is_none()
            && let Some(path) = normalize_dir(PathBuf::from(arg))
        {
            base_path = Some(path);
        }
    }

    LaunchOptions {
        base_path: base_path.clone().unwrap_or_else(home_dir),
        open_path,
        base_path_explicit: base_path.is_some(),
        start_in_grep,
        show_help,
        show_version,
    }
}

fn print_help() {
    println!(
        "fff-gpui {version}\n\n\
Usage:\n  fff-gpui [OPTIONS] [PATH]\n\n\
Options:\n  --grep            Start in grep mode\n  --open <PATH>     Open a specific path\n  -h, --help        Show this help text\n  -V, --version     Show version information",
        version = env!("CARGO_PKG_VERSION")
    );
}

#[cfg(target_os = "macos")]
fn make_dockless() {
    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicyAccessory};

    unsafe {
        let app = NSApp();
        app.setActivationPolicy_(NSApplicationActivationPolicyAccessory);
    }
}

#[cfg(not(target_os = "macos"))]
fn make_dockless() {}

// Register the built-in key bindings for the picker and text field actions.
fn bind_base_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Quit, None),
        KeyBinding::new("enter", OpenSelected, None),
        KeyBinding::new("return", OpenSelected, None),
        KeyBinding::new("up", SelectPrev, None),
        KeyBinding::new("down", SelectNext, None),
        KeyBinding::new("tab", ToggleSelectedAndAdvance, None),
        KeyBinding::new("ctrl-a", ToggleSelectAll, None),
        KeyBinding::new("shift-tab", ShiftTab, None),
        KeyBinding::new("ctrl-up", CyclePreviousQuery, None),
        KeyBinding::new("ctrl-u", PreviewScrollUp, None),
        KeyBinding::new("ctrl-d", PreviewScrollDown, None),
        KeyBinding::new("ctrl-f", SwitchFiles, None),
        KeyBinding::new("ctrl-g", SwitchGrep, None),
        KeyBinding::new("backspace", FieldBackspace, None),
        KeyBinding::new("delete", FieldDelete, None),
        KeyBinding::new("left", FieldLeft, None),
        KeyBinding::new("right", FieldRight, None),
        KeyBinding::new("shift-left", FieldSelectLeft, None),
        KeyBinding::new("shift-right", FieldSelectRight, None),
        KeyBinding::new("cmd-a", FieldSelectAll, None),
        KeyBinding::new("cmd-v", FieldPaste, None),
        KeyBinding::new("cmd-c", FieldCopy, None),
        KeyBinding::new("cmd-x", FieldCut, None),
        KeyBinding::new("home", FieldHome, None),
        KeyBinding::new("end", FieldEnd, None),
    ]);
}

fn bind_runtime_keys(cx: &mut App, config: &AppConfig) {
    if let Some(binding) = config.global_keybind.as_deref()
        && let Ok(Some(_)) = hotkey::parse_hotkey(Some(binding))
    {
        cx.bind_keys([KeyBinding::new(binding, ToggleWindow, None)]);
    }
}

fn apply_key_bindings(cx: &mut App, config: &AppConfig, replace: bool) -> bool {
    match hotkey::parse_hotkey(config.global_keybind.as_deref()) {
        Ok(_) => {
            if replace {
                cx.clear_key_bindings();
            }
            bind_base_keys(cx);
            bind_runtime_keys(cx, config);
            true
        }
        Err(err) => {
            if !replace {
                cx.clear_key_bindings();
                bind_base_keys(cx);
            }
            info!(error = %err, "skipping global hotkey binding");
            false
        }
    }
}

fn register_global_hotkey(state: &mut RuntimeConfig, config: &AppConfig) {
    let Ok(Some(hotkey)) = hotkey::parse_hotkey(config.global_keybind.as_deref()) else {
        if let Some(manager) = &state.hotkey_manager
            && let Some(existing) = state.registered_hotkey.take()
        {
            let _ = manager.unregister(existing);
        }
        return;
    };

    if state.hotkey_manager.is_none() {
        state.hotkey_manager = GlobalHotKeyManager::new().ok();
    }

    let Some(manager) = &state.hotkey_manager else {
        info!("global hotkey manager unavailable");
        return;
    };

    if state.registered_hotkey == Some(hotkey) {
        return;
    }

    let previous = state.registered_hotkey.take();
    if let Some(existing) = previous {
        let _ = manager.unregister(existing);
    }

    match manager.register(hotkey) {
        Ok(_) => {
            state.registered_hotkey = Some(hotkey);
            info!(hotkey = %hotkey, "registered global toggle hotkey");
        }
        Err(err) => {
            if let Some(existing) = previous {
                let _ = manager.register(existing);
                state.registered_hotkey = Some(existing);
            }
            info!(error = %err, hotkey = %hotkey, "failed to register global hotkey");
        }
    }
}

fn open_config_file(runtime: &Arc<Mutex<RuntimeConfig>>) {
    let (path, config) = match runtime.lock() {
        Ok(state) => (state.config_path.clone(), state.config.clone()),
        Err(_) => (config::active_config_path(), AppConfig::default()),
    };

    if let Err(err) = config::ensure_config_file(&path, &config) {
        info!(error = %err, path = %path.display(), "failed to ensure config file");
        return;
    }

    match editor::open_in_editor(&path, None) {
        Ok(child) => {
            info!(pid = child.id(), path = %path.display(), "opened config file");
        }
        Err(err) => {
            info!(error = %err, path = %path.display(), "failed to open config file");
        }
    }
}

// Open the main picker window centered on the primary display.
fn open_window(session: PickerSession, runtime_config: &Arc<Mutex<RuntimeConfig>>, cx: &mut App) {
    let config = runtime_config
        .lock()
        .map(|state| state.config.clone())
        .unwrap_or_default();
    theme::sync_from_config(&config, cx.window_appearance(), cx);

    let base_path = session.base_path;
    let shared = session.shared;
    let enable_content_indexing = session.enable_content_indexing;
    let start_in_grep = session.start_in_grep;
    let responder = session.responder;
    let window_width = config.window_width;
    let window_height = config.window_height;
    let bounds = cx
        .primary_display()
        .map(|d| {
            let db = d.bounds();
            let x = db.origin.x + (db.size.width - px(window_width)) / 2.0;
            let y = db.origin.y + (db.size.height - px(window_height)) / 3.0;
            Bounds {
                origin: point(x, y),
                size: size(px(window_width), px(window_height)),
            }
        })
        .unwrap_or(Bounds {
            origin: point(px(400.0), px(200.0)),
            size: size(px(window_width), px(window_height)),
        });

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            is_resizable: false,
            kind: WindowKind::PopUp,
            ..WindowOptions::default()
        },
        |window, cx| {
            let view = cx.new(|cx| {
                FffPicker::new(
                    base_path.clone(),
                    shared.clone(),
                    enable_content_indexing,
                    start_in_grep,
                    responder.clone(),
                    cx,
                )
            });
            view.update(cx, |picker, cx| {
                picker.install_focus_lost_dismiss(window, cx);
            });
            let focus = view.read(cx).text_field_focus_handle(cx);
            window.focus(&focus);
            view
        },
    )
    .expect("failed to open fff window");
}

fn close_all_windows(cx: &mut App) {
    let windows = cx.windows();
    for window in windows {
        let _ = window.update(cx, |_, window, _| {
            window.remove_window();
        });
    }
}

fn toggle_picker(
    session: &Arc<Mutex<PickerSession>>,
    runtime_config: &Arc<Mutex<RuntimeConfig>>,
    cx: &mut App,
) {
    if cx.windows().is_empty() {
        let session = snapshot_session(session);
        info!(base_path = %session.base_path.display(), "opening picker window");
        open_window(session, runtime_config, cx);
    } else {
        info!("closing picker window(s)");
        close_all_windows(cx);
    }
}

fn snapshot_session(session: &Arc<Mutex<PickerSession>>) -> PickerSession {
    session
        .lock()
        .map(|session| session.clone())
        .unwrap_or_else(|_| PickerSession::new(home_dir(), false, false))
}

fn one_shot_session(
    path: PathBuf,
    sessions: &Arc<Mutex<HashMap<PathBuf, PickerSession>>>,
    start_in_grep: bool,
    responder: Option<ResponderArc>,
) -> PickerSession {
    let fallback_path = path.clone();
    let mut session = sessions
        .lock()
        .map(|mut sessions| {
            sessions
                .entry(path.clone())
                .or_insert_with(|| PickerSession::new(path, true, start_in_grep))
                .clone()
        })
        .unwrap_or_else(|_| PickerSession::new(fallback_path, true, start_in_grep));
    session.start_in_grep = start_in_grep;
    session.responder = responder;
    session
}

fn show_or_focus_picker(
    session: &Arc<Mutex<PickerSession>>,
    runtime_config: &Arc<Mutex<RuntimeConfig>>,
    cx: &mut App,
) {
    if cx.windows().is_empty() {
        let session = snapshot_session(session);
        info!(base_path = %session.base_path.display(), "reopening picker window");
        open_window(session, runtime_config, cx);
    }
}

fn wrap_responder(stream: Option<service::ClientStream>) -> Option<ResponderArc> {
    stream.map(|s| Arc::new(Mutex::new(Some(s))))
}

fn handle_service_command(
    command: ServiceCommand,
    stream: Option<service::ClientStream>,
    session: &Arc<Mutex<PickerSession>>,
    one_shot_sessions: &Arc<Mutex<HashMap<PathBuf, PickerSession>>>,
    runtime_config: &Arc<Mutex<RuntimeConfig>>,
    cx: &mut App,
) {
    let responder = wrap_responder(stream);
    match command {
        ServiceCommand::ShowPicker => {
            debug!("received show-picker service command");
            show_or_focus_picker(session, runtime_config, cx)
        }
        ServiceCommand::ToggleWindow => {
            debug!("received toggle-window service command");
            toggle_picker(session, runtime_config, cx)
        }
        ServiceCommand::OpenPath { path, in_grep } => {
            debug!(path = %path.display(), in_grep, "received open-path service command");
            let (should_replace, session_snapshot) = match session.lock() {
                Ok(mut current) => {
                    let changed = current.base_path != path;
                    if changed {
                        *current = PickerSession::new(path.clone(), true, in_grep);
                    } else {
                        current.start_in_grep = in_grep;
                    }
                    current.responder = responder.clone();
                    (changed, current.clone())
                }
                Err(_) => {
                    let mut s = PickerSession::new(path.clone(), true, in_grep);
                    s.responder = responder.clone();
                    (true, s)
                }
            };

            if cx.windows().is_empty() {
                open_window(session_snapshot, runtime_config, cx);
            } else if should_replace {
                close_all_windows(cx);
                open_window(session_snapshot, runtime_config, cx);
            }
        }
        ServiceCommand::OpenOneShot { path, in_grep } => {
            debug!(path = %path.display(), in_grep, "received one-shot open service command");
            open_window(
                one_shot_session(path, one_shot_sessions, in_grep, responder),
                runtime_config,
                cx,
            );
        }
        ServiceCommand::OpenConfig => {
            debug!("received open-config service command");
            open_config_file(runtime_config);
        }
        ServiceCommand::Quit => {
            debug!("received quit service command");
            cx.quit();
        }
    }
}

async fn drive_service_commands(
    rx: async_channel::Receiver<CommandEnvelope>,
    session: Arc<Mutex<PickerSession>>,
    one_shot_sessions: Arc<Mutex<HashMap<PathBuf, PickerSession>>>,
    runtime_config: Arc<Mutex<RuntimeConfig>>,
    app: AsyncApp,
) {
    while let Ok((command, stream)) = rx.recv().await {
        let _ = app.update(|app| {
            handle_service_command(
                command,
                stream,
                &session,
                &one_shot_sessions,
                &runtime_config,
                app,
            )
        });
    }
}

// Launch the resident GPUI service.
fn main() {
    log::init_tracing();

    let launch = parse_launch_options();
    if launch.show_help {
        print_help();
        return;
    }
    if launch.show_version {
        println!("fff-gpui {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let base_path = launch.base_path.clone();
    let picker_session = Arc::new(Mutex::new(PickerSession::new(
        base_path.clone(),
        launch.base_path_explicit,
        launch.start_in_grep,
    )));
    let one_shot_sessions = Arc::new(Mutex::new(HashMap::new()));
    let loaded_config = match config::load_active_config() {
        Ok(loaded) => loaded,
        Err(err) => {
            info!(error = %err, "failed to load config; falling back to defaults");
            config::LoadedConfig {
                path: config::active_config_path(),
                config: AppConfig::default(),
            }
        }
    };
    let runtime_config = Arc::new(Mutex::new(RuntimeConfig {
        config: loaded_config.config.clone(),
        config_path: loaded_config.path.clone(),
        hotkey_manager: None,
        registered_hotkey: None,
    }));
    let shell = std::env::var("SHELL").ok();
    let home = std::env::var("HOME").ok();
    let editor = std::env::var("EDITOR").ok();
    let visual = std::env::var("VISUAL").ok();
    let path_env = std::env::var("PATH").ok();
    let cwd = std::env::current_dir().ok();

    info!(
        base_path = %base_path.display(),
        config_path = %loaded_config.path.display(),
        global_keybind = ?loaded_config.config.global_keybind,
        "starting fff-gpui"
    );
    debug!(
        shell = ?shell,
        home = ?home,
        editor = ?editor,
        visual = ?visual,
        path_env = ?path_env,
        cwd = ?cwd,
        "startup environment"
    );

    let forward_command = match launch.open_path.clone() {
        Some(path) => ServiceCommand::OpenOneShot {
            path,
            in_grep: launch.start_in_grep,
        },
        None => ServiceCommand::OpenPath {
            path: base_path.clone(),
            in_grep: launch.start_in_grep,
        },
    };

    match forward_to_running_instance(&forward_command)
        .expect("failed to forward launch request to existing service")
    {
        ForwardOutcome::NoDaemon => {
            info!("no resident service; this process will become the daemon");
        }
        ForwardOutcome::Picked(entries) => {
            info!(count = entries.len(), "received pick response from daemon");
            for entry in entries {
                let goto = entry.line.zip(entry.column);
                match editor::open_in_editor(&entry.path, goto) {
                    Ok(mut child) => {
                        let _ = child.wait();
                    }
                    Err(err) => {
                        eprintln!("fff-gpui: failed to open {}: {err}", entry.path.display())
                    }
                }
            }
            return;
        }
    }

    let (service_tx, service_rx) = async_channel::unbounded::<CommandEnvelope>();
    start_listener(service_tx.clone()).expect("failed to start launch request listener");
    info!("resident service listener started");

    let app = Application::new().with_assets(Assets);
    let reopen_session = picker_session.clone();
    let reopen_runtime_config = runtime_config.clone();
    app.on_reopen(move |cx| show_or_focus_picker(&reopen_session, &reopen_runtime_config, cx));

    app.run(move |cx: &mut App| {
        FontAssets::load_fonts(cx).expect("failed to load bundled fonts");
        make_dockless();
        hotkey::install_event_handler(service_tx.clone());
        if let Ok(state) = runtime_config.lock() {
            theme::sync_from_config(&state.config, cx.window_appearance(), cx);
            let _ = apply_key_bindings(cx, &state.config, false);
        }
        cx.on_action({
            let picker_session = picker_session.clone();
            let runtime_config = runtime_config.clone();
            move |_: &ToggleWindow, cx| toggle_picker(&picker_session, &runtime_config, cx)
        });
        {
            let runtime_config = runtime_config.clone();
            cx.on_action(move |_: &OpenConfig, _cx| {
                info!("config action invoked");
                open_config_file(&runtime_config);
            });
        }
        cx.spawn({
            let runtime_config = runtime_config.clone();
            async move |cx: &mut AsyncApp| {
                let _ = cx.update(|_app| {
                    if let Ok(mut state) = runtime_config.lock() {
                        let config = state.config.clone();
                        register_global_hotkey(&mut state, &config);
                    }
                });
            }
        })
        .detach();
        if let Some(path) = launch.open_path.clone() {
            open_window(
                one_shot_session(path, &one_shot_sessions, launch.start_in_grep, None),
                &runtime_config,
                cx,
            );
        } else {
            info!("launching without initial picker window");
        }
        menubar::install(service_tx.clone());

        cx.spawn(move |cx: &mut AsyncApp| {
            drive_service_commands(
                service_rx,
                picker_session.clone(),
                one_shot_sessions.clone(),
                runtime_config.clone(),
                cx.clone(),
            )
        })
        .detach();
    });
}
