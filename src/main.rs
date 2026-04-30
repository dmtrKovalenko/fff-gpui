mod editor;
mod log;
mod path_shortening;
mod picker;
mod preview;
mod text_field;
mod theme;

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::*;

use picker::{FffPicker, OpenSelected, Quit, SelectNext, SelectPrev};
use text_field::{
    FieldBackspace, FieldCopy, FieldCut, FieldDelete, FieldEnd, FieldHome, FieldLeft, FieldPaste,
    FieldRight, FieldSelectAll, FieldSelectLeft, FieldSelectRight,
};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const WINDOW_WIDTH: f32 = 960.0;
const WINDOW_HEIGHT: f32 = 520.0;

// Resolve the base directory from argv[1] or the current working directory.
fn resolve_base_path() -> PathBuf {
    std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

// Register key bindings for the picker and text field actions.
fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Quit, None),
        KeyBinding::new("enter", OpenSelected, None),
        KeyBinding::new("return", OpenSelected, None),
        KeyBinding::new("up", SelectPrev, None),
        KeyBinding::new("down", SelectNext, None),
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

// Open the main picker window centered on the primary display.
fn open_window(base_path: PathBuf, cx: &mut App) {
    let bounds = cx
        .primary_display()
        .map(|d| {
            let db = d.bounds();
            let x = db.origin.x + (db.size.width - px(WINDOW_WIDTH)) / 2.0;
            let y = db.origin.y + (db.size.height - px(WINDOW_HEIGHT)) / 3.0;
            Bounds {
                origin: point(x, y),
                size: size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
            }
        })
        .unwrap_or(Bounds {
            origin: point(px(400.0), px(200.0)),
            size: size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
        });

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(8.0), px(8.0))),
                ..Default::default()
            }),
            is_resizable: false,
            ..WindowOptions::default()
        },
        |window, cx| {
            let view = cx.new(|cx| FffPicker::new(base_path, cx));
            let focus = view.read(cx).text_field_focus_handle(cx);
            window.focus(&focus);
            view
        },
    )
    .expect("failed to open fff window");
}

// Launch the GPUI application.
fn main() {
    let base_path = resolve_base_path();

    Application::new().run(|cx: &mut App| {
        cx.activate(true);
        cx.on_action(|_: &Quit, cx| cx.quit());
        bind_keys(cx);
        open_window(base_path, cx);
    });
}
