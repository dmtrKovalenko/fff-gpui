# fff-gpui

A fast, keyboard-driven file finder for macOS built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) — the same UI framework that powers [Zed](https://zed.dev). It runs as a system-wide overlay you can summon instantly with a keybind, and integrates seamlessly into Zed as a custom task.

Under the hood it uses [fff](https://crates.io/crates/fff-search) for fuzzy file search and grep, with frecency-based ranking so the files you actually use rise to the top.

<img width="1072" height="633" alt="Screenshot 2026-05-01 at 4 44 15 PM" src="https://github.com/user-attachments/assets/4ad54fdc-5f0b-4ecf-908e-1bc3fe27f4ae" />

## Features

- Fuzzy file search and grep across your project
- Frecency ranking — frequently and recently opened files are prioritised
- Syntax-highlighted file preview
- Global keybind support for system-wide access
- Deep Zed integration via custom tasks — works across all projects
- Launch at login support

## Zed integration

This is the recommended way to use fff-gpui. Add the following to your Zed config files and replace `/path/to/fff-gpui` with the actual path to your binary.

**`~/.config/zed/tasks.json`**
```json
{
  "label": "Open fff-gpui",
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
}
```

**`~/.config/zed/keymap.json`**
```json
{
  "context": "Workspace",
  "bindings": {
    "cmd-p": [
      "task::Spawn", 
      { "task_name": "Open fff-gpui" }]
  }
}
```

This opens fff-gpui scoped to the current project root. Selected files open directly in Zed.

## Installation

**Requirements:**
- macOS (Apple Silicon and Intel)
- Latest stable Rust via [rustup](https://rustup.rs)
- Xcode Command Line Tools (`xcode-select --install`)
- CMake ([required by wasmtime](https://docs.rs/wasmtime-c-api-impl/latest/wasmtime_c_api/))
- Zig 0.16.0 ([required by zlob](https://crates.io/crates/zlob))

To compile without Zig, disable zlob in the `Cargo.toml`. This will lead to slightly slower performance, but it's not required for the app to work.

```toml
+ fff-search = "0.6"
+ fff-query-parser = "0.6"
- fff-search = { version = "0.6", features = ["zlob"] }
- fff-query-parser = { version = "0.6", features = ["zlob"] }
```


**Build:**
```sh
git clone https://github.com/th0jensen/fff-gpui
cd fff-gpui
cargo build --release
```

The binary will be at `target/release/fff-gpui`. You can move it anywhere on your `$PATH` or reference it directly in your config.

Having trouble building? Check Zed's [macOS troubleshooting guide](https://zed.dev/docs/development/macos#troubleshooting) — the build requirements are the same.

## Global keybind

fff-gpui can also run as a background daemon with a system-wide hotkey, independent of Zed.

Set a keybind in `~/.config/fff-gpui/config.toml`:

```toml
launch_at_login = true
sync_zed_settings = true
global_keybind = "cmd+shift+f"
```

When `sync_zed_settings` is enabled, fff-gpui reads Zed's `settings.json` and mirrors the UI font, buffer font, light/dark theme selection, and theme colors from any installed Zed theme.

Zed themes are discovered from your local Zed installation, including extension themes under `~/Library/Application Support/Zed/extensions/installed/` on macOS.

Then launch the app once in background mode:

```sh
fff-gpui -d
```

From then on it will start automatically at login and be available system-wide.

## License

MIT
