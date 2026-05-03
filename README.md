# fff-gpui

A fast, keyboard-driven file finder for macOS built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) — the same UI framework that powers [Zed](https://zed.dev). It runs as a system-wide overlay you can summon instantly with a keybind, and integrates seamlessly into Zed as a custom task.

Under the hood it uses [fff](https://crates.io/crates/fff-search) for fuzzy file search and grep, with frecency-based ranking so the files you actually use rise to the top.

<img width="1072" height="633" alt="Screenshot 2026-05-03 at 3 06 14 PM" src="https://github.com/user-attachments/assets/db45bdc6-933e-4af8-b7df-b285317f0cc2" />

## Features

- Fuzzy file search and grep across your project
- Frecency ranking — frequently and recently opened files are prioritised
- Syntax-highlighted file preview
- Global keybind support for system-wide access
- Deep Zed integration via custom tasks — works across all projects

## Installation

### Homebrew (recommended)

```sh
brew tap th0jensen/fff-gpui
brew install fff-gpui
brew services start fff-gpui
```

## Configuration

Set options in `~/.config/fff-gpui/config.toml`:

```toml
editor = "zed"
sync_zed_settings = true
global_keybind = "hyper+f"
window_width = 960.0
window_height = 520.0
picker_pane_width = 430.0

[font]
ui_family = ".SystemUIFont"
buffer_family = "UbuntuMono Nerd Font"
ui_size = 16.0
buffer_size = 15.0

[theme]
name = "One Dark"
```

`editor` is a fallback for the resident Homebrew service, which does not inherit your shell environment. If `EDITOR` or `VISUAL` is present in the current process, those still win, so custom tasks and other integrations can keep overriding it naturally.

When `sync_zed_settings` is enabled, fff-gpui reads Zed's `settings.json` and mirrors the UI font, buffer font, font sizes, light/dark theme selection, and theme colors — from the bundled Zed themes plus any installed or local Zed theme.

Explicit config values still win, so you can keep Zed sync enabled and override just the theme, fonts, sizes, or specific colors when needed. In practice, `[theme].name` overrides Zed's chosen theme, and `[font]` overrides the synced font families and sizes.

For `global_keybind`, `hyper` is accepted as a shorthand for `shift+control+alt+super`.

Zed themes are discovered from the bundled theme set, your local Zed installation, and extension themes under `~/Library/Application Support/Zed/extensions/installed/`.

## Running

Launch fff-gpui once to start it as a background service with your global keybind:

```sh
fff-gpui
```

If installed via Homebrew, `brew services start fff-gpui` handles this and re-launches it at login automatically.

## Zed integration

This is the recommended way to use fff-gpui within a project. Add the following to your Zed config files and replace `/path/to/fff-gpui` with the actual path to your binary.

**`~/.config/zed/tasks.json`**
```json
[
  {
    "label": "fff-gpui: Files",
    "command": "/path/to/fff-gpui --open .",
    "env": { "EDITOR": "zed" },
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "reveal": "never",
    "reveal_target": "dock",
    "hide": "always",
    "shell": "system",
    "show_summary": false,
    "show_command": false,
    "save": "none"
  },
  {
    "label": "fff-gpui: Grep",
    "command": "/path/to/fff-gpui --open . --grep",
    "env": { "EDITOR": "zed" },
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "reveal": "never",
    "reveal_target": "dock",
    "hide": "always",
    "shell": "system",
    "show_summary": false,
    "show_command": false,
    "save": "none"
  }
]
```

**`~/.config/zed/keymap.json`**
```json
{
  "context": "Workspace",
  "bindings": {
    "cmd-k cmd-p": ["task::Spawn", { "task_name": "fff-gpui: Files" }],
    "cmd-k cmd-f": ["task::Spawn", { "task_name": "fff-gpui: Grep" }]
  }
}
```

This opens fff-gpui scoped to the current project root. `cmd-k cmd-p` launches in file-search mode, `cmd-k cmd-f` launches directly in grep mode. Selected files open in Zed; with grep, the editor jumps to the matched line.

## Build from source

**Requirements:**
- macOS (Apple Silicon and Intel)
- Latest stable Rust via [rustup](https://rustup.rs)
- Xcode Command Line Tools (`xcode-select --install`)
- CMake ([required by wasmtime](https://docs.rs/wasmtime-c-api-impl/latest/wasmtime_c_api/))
- Zig 0.16.0 ([required by zlob](https://crates.io/crates/zlob))

To compile without Zig, disable zlob in `Cargo.toml`. This will lead to slightly slower performance, but it's not required for the app to work.

```toml
+ fff-search = "0.6"
+ fff-query-parser = "0.6"
- fff-search = { version = "0.6", features = ["zlob"] }
- fff-query-parser = { version = "0.6", features = ["zlob"] }
```

```sh
git clone https://github.com/th0jensen/fff-gpui
cd fff-gpui
cargo build --release
```

The binary will be at `target/release/fff-gpui`. You can move it anywhere on your `$PATH` or reference it directly in your config.

Having trouble building? Check Zed's [macOS troubleshooting guide](https://zed.dev/docs/development/macos#troubleshooting) — the build requirements are the same.

## License

MIT
