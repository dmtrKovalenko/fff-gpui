# fff-gpui Implementation Plan

## Overview

A standalone GPUI app that launches a Zed-style file picker over a given directory, lets the user fuzzy-search files, and opens the selected file in `$EDITOR` on Enter. It exits after opening.

---

## Architecture

```
main()
 └─ App::new() → window → FffPicker (Render)
      ├─ SearchInput  (custom TextField, adapted from crabdash)
      ├─ ResultsList  (uniform_list for virtualisation)
      └─ StatusBar    (match count)
```

**State lives in one root struct** `FffPicker` that holds:

| Field | Type | Purpose |
|---|---|---|
| `shared_picker` | `SharedPicker` | Thread-safe FilePicker access |
| `shared_frecency` | `SharedFrecency` | Frecency DB access |
| `query` | `String` | Current text field value |
| `results` | `Vec<FileItemSnapshot>` | Cached snapshot of latest search |
| `selected` | `usize` | Highlighted row index |
| `scan_done` | `bool` | Whether initial scan completed |
| `focus_handle` | `FocusHandle` | Keyboard routing |
| `list_scroll` | `UniformListScrollHandle` | Scroll state |
| `base_path` | `PathBuf` | Root directory |

`FileItemSnapshot` is a small owned struct cloned from `FileItem` refs (path strings + frecency score) so results are `Send` across the async boundary.

---

## Cargo.toml Changes

```toml
[dependencies]
gpui        = { version = "*" }
fff-search  = "0.6"
smol        = "2"              # async executor matching gpui's model
```

Fonts: embed JetBrains Mono Nerd Regular from crabdash (copy the font bytes pattern).

---

## File Structure

```
src/
  main.rs          – app init, window, key bindings, entry point
  picker.rs        – FffPicker view (Render impl, search logic)
  text_field.rs    – adapted from crabdash (IME-safe text input)
  results_list.rs  – uniform_list row renderer
  theme.rs         – color constants (mirrors crabdash palette)
  editor.rs        – $EDITOR launch + frecency tracking
```

---

## Phase 1 — Scaffolding & Window

**`main.rs`:**
- Parse `argv[1]` as base directory, default to `std::env::current_dir()`
- `App::new().run(|cx| { ... })` (smol executor)
- Register fonts (JetBrains Mono Nerd via `cx.text_system().add_fonts()`)
- Bind keys globally:
  - `escape` → `Quit`
  - `enter` → `OpenSelected`
  - `up` → `SelectPrev`
  - `down` → `SelectNext`
- Open a 640×400 centered window, `appears_transparent: false`, no native title bar (or custom 28 px bar)
- Push `FffPicker` as the root view

**Window dimensions:** Fixed 640×400 px. GPUI does not expose "center on display" directly, so use `WindowOptions::bounds` with half of the display size (read via `cx.displays()[0].bounds()`).

---

## Phase 2 — FilePicker Integration

**In `FffPicker::new()`:**

```rust
let shared_picker = SharedPicker::default();
let shared_frecency = SharedFrecency::default();

// FrecencyTracker stored in ~/.local/share/fff/frecency.lmdb
let db_path = dirs::data_dir().unwrap().join("fff/frecency.lmdb");
let tracker = FrecencyTracker::new(&db_path, false)?;
shared_frecency.init(tracker)?;

// Background scan
let sp = shared_picker.clone();
cx.background_spawn(async move {
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: base_path.to_string_lossy().into(),
        enable_mmap_cache: false,
        enable_content_indexing: false,
        mode: FFFMode::Neovim,
        cache_budget: None,
        watch: false,         // no watcher needed for a single-shot picker
    })?;
    picker.collect_files()?;
    *sp.write()? = Some(picker);
    Ok::<_, anyhow::Error>(())
}).detach();
```

After scan, use a `cx.spawn` watcher (poll `shared_picker.wait_for_scan()`) to flip `scan_done = true` and run the initial (empty-query) search.

---

## Phase 3 — Search Loop

**`FffPicker::run_search(&mut self, cx: &mut Context<Self>)`:**

```rust
let query_str = self.query.clone();
let sp = self.shared_picker.clone();

cx.spawn(async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
    let guard = sp.read()?;
    let Some(picker) = guard.as_ref() else { return Ok(()); };

    let parser = QueryParser::default();
    let query = parser.parse(&query_str);

    let results: Vec<FileItemSnapshot> = picker
        .fuzzy_search(&query, None, FuzzySearchOptions {
            max_threads: 4,
            pagination: PaginationArgs { offset: 0, limit: 200 },
            ..Default::default()
        })
        .items
        .iter()
        .map(|fi| FileItemSnapshot::from(*fi, picker.base_path()))
        .collect();

    this.update(cx, |this, cx| {
        this.results = results;
        this.selected = 0;
        cx.notify();
    })
}).detach();
```

**Debounce:** Run search immediately on every keystroke (fff-search is fast, in-memory). If the repo is huge (>100k files) add a 50 ms debounce using `cx.spawn` with a timer.

---

## Phase 4 — UI Rendering

### Search Bar

```
┌──────────────────────────────────────────────────────────┐
│  🔍  query text here                                      │
└──────────────────────────────────────────────────────────┘
```

- Full-width `div`, height 44 px
- Left-padded magnifier icon (Lucide `Search`, 14 px)
- `TextField` entity occupying remaining width
- Bottom border `rgb(0x2F2F31)` to separate from list
- Focused blue border on the input element mirrors crabdash pattern

### Results List

Use `uniform_list` (GPUI built-in virtual scroller) to handle thousands of rows without DOM overhead:

```rust
uniform_list(cx.entity(), "results", self.results.len(), {
    let results = self.results.clone();
    let selected = self.selected;
    move |range, _window, cx| {
        range.map(|i| render_row(&results[i], i == selected, cx)).collect()
    }
})
.flex_1()
.w_full()
```

**Row layout (32 px tall):**

```
│  src/picker.rs          src/          │
│  ←filename (white)→  ←dir (gray)→    │
```

- Filename: `rgb(0xFFFFFF)`, `text_sm()`
- Directory: `rgb(0x8E8E93)`, `text_xs()`, right-aligned or after a spacer
- Selected row background: `rgb(0x2C3F59)` (blue-tinted, Zed style)
- Hover: `rgb(0x2A2A2C)`
- Padding: `px_x(12)`, items centered

Match highlights: collect byte ranges from `Score` if available and render highlighted spans in `rgb(0x4A9EFF)`.

### Status Bar

Single `div`, height 28 px, border top, shows:
- `{matched} of {total} files` on left
- `↑↓ navigate  ↵ open  esc quit` hint on right (dimmed)
- Colors: `rgb(0x6C6C70)` text on `rgb(0x18181A)` bg

---

## Phase 5 — Keyboard Handling

Actions defined at the top of `picker.rs`:

```rust
actions!(fff_picker, [Quit, OpenSelected, SelectNext, SelectPrev]);
```

`FffPicker::render()` attaches handlers:

```rust
div()
    .track_focus(&self.focus_handle)
    .on_action(cx.listener(Self::on_quit))
    .on_action(cx.listener(Self::on_open_selected))
    .on_action(cx.listener(Self::on_select_next))
    .on_action(cx.listener(Self::on_select_prev))
```

`SelectNext`/`SelectPrev` clamp to `[0, results.len() - 1]` and call `self.list_scroll.scroll_to_item(self.selected)`.

The `TextField` must forward `Up`/`Down`/`Enter`/`Escape` to the parent instead of consuming them. Implement this by checking the key in `TextField::key_down` and calling the parent action when the key is not a text-editing key.

---

## Phase 6 — Opening the File

```rust
pub fn open_in_editor(path: &Path, location: Option<Location>) -> Result<()> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".into());

    let mut cmd = std::process::Command::new(&editor);

    // Handle line:col for editors that support it
    if let Some(loc) = location {
        match editor_kind(&editor) {
            EditorKind::Vim | EditorKind::Nvim => {
                cmd.arg(format!("+{}", loc.line)).arg(path);
            }
            EditorKind::VSCode => {
                cmd.arg("--goto").arg(format!("{}:{}:{}", path.display(), loc.line, loc.col));
            }
            EditorKind::Other => { cmd.arg(path); }
        }
    } else {
        cmd.arg(path);
    }

    cmd.spawn()?;
    Ok(())
}
```

Called from `on_open_selected`:

```rust
fn on_open_selected(&mut self, _: &OpenSelected, _window: &mut Window, cx: &mut Context<Self>) {
    let Some(item) = self.results.get(self.selected) else { return; };
    let path = item.absolute_path.clone();

    // Track frecency
    if let Ok(guard) = self.shared_frecency.read() {
        if let Some(tracker) = guard.as_ref() {
            let _ = tracker.track_access(&path);
        }
    }

    let _ = open_in_editor(&path, None);
    cx.quit();
}
```

---

## Phase 7 — Polish

- **Empty state**: When `scan_done` is false, show "Indexing…" text centered in the list area. When query is non-empty but `results` is empty, show "No files matched".
- **Window focus**: Call `window.activate()` on startup so the window is front and keyboard works immediately.
- **Quit on focus loss**: Optional — `on_blur` at the window level calls `cx.quit()` (fzf behaviour). Make this opt-in via a `--stay` flag.
- **CLI args**: Accept `[directory]` positional arg. Validate it is a readable directory, else fall back to cwd.

---

## Color Reference

| Token | Value |
|---|---|
| Background | `rgb(0x1C1C1E)` |
| Input bg | `rgb(0x232326)` |
| Border | `rgb(0x2F2F31)` |
| Selected row | `rgb(0x2C3F59)` |
| Hover row | `rgb(0x2A2A2C)` |
| Text primary | `rgb(0xFFFFFF)` |
| Text secondary | `rgb(0x8E8E93)` |
| Text dim | `rgb(0x6C6C70)` |
| Accent (blue) | `rgb(0x0A84FF)` |
| Match highlight | `rgb(0x4A9EFF)` |
| Status bar bg | `rgb(0x18181A)` |

---

## Key Constraints & Gotchas

1. **`FFFQuery` lifetime**: `parser.parse(query)` borrows `query`. The query string must outlive the search call. Keep `query_str` as a local `String` in the async block — don't try to store `FFFQuery` in state.

2. **`uniform_list` requires item count up-front**: Store `results.len()` in state before rendering; the closure must be `'static + Fn` so clone what you need into it.

3. **`SharedPicker` RwLock on background thread**: The scan runs on `cx.background_spawn`. The GPUI UI thread must not hold the write lock. Use `WeakEntity` pattern (same as crabdash) to update state after scan completes.

4. **TextField key routing**: GPUI routes keys to the focused element first. The `TextField` will consume `ArrowUp`/`ArrowDown` unless you explicitly do not bind them there and let them bubble to the parent `FffPicker` action handlers.

5. **`cx.quit()` vs `std::process::exit()`**: Prefer `cx.quit()` so GPUI can clean up. The `open_in_editor` spawns the editor as a detached child process before quitting.

6. **macOS sandboxing**: No bundle needed for a CLI tool. Run directly from terminal; `$EDITOR` will be inherited from the shell environment.

---

## Implementation Order

1. `theme.rs` — color constants
2. `main.rs` — window + font registration + key bindings + app run
3. `picker.rs` — `FffPicker` struct, `Render` impl (static skeleton first)
4. `text_field.rs` — port from crabdash, strip machine-specific cruft
5. `editor.rs` — `open_in_editor` + `EditorKind`
6. Wire background scan → `scan_done` → initial render
7. Wire `TextField` changes → `run_search` → `results` update
8. `results_list.rs` — `uniform_list` rows
9. Status bar
10. Keyboard actions (`SelectNext`, `SelectPrev`, `OpenSelected`, `Quit`)
11. Polish: empty states, focus-on-launch, quit-on-blur
