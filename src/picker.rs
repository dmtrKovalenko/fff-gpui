use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use fff_query_parser::{FileSearchConfig, QueryParser};
use fff_search::{
    FFFMode, FilePickerOptions, FuzzySearchOptions, GrepMode, GrepSearchOptions, PaginationArgs,
    SharedFrecency, SharedPicker, SharedQueryTracker, file_picker::FilePicker,
    frecency::FrecencyTracker, git::format_git_status_opt, grep::has_regex_metacharacters,
    query_tracker::QueryTracker,
};
use gpui::prelude::*;
use gpui::*;
use tracing::{debug, error, info, trace, warn};

use crate::editor;
use crate::log;
use crate::path_shortening::PathShortenStrategy;
use crate::preview::{self, HighlightedLine};
use crate::text_field::TextField;
use crate::theme;

actions!(
    fff_picker,
    [
        Quit,
        OpenSelected,
        SelectNext,
        SelectPrev,
        ToggleSelected,
        CycleGrepMode,
        CyclePreviousQuery,
        PreviewScrollUp,
        PreviewScrollDown,
        SwitchFiles,
        SwitchGrep,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchView {
    Files,
    Grep,
}

#[derive(Clone, Debug, Default)]
pub struct PickerSharedState {
    pub shared_picker: SharedPicker,
    pub shared_frecency: SharedFrecency,
    pub shared_query_tracker: SharedQueryTracker,
}

// Return a sensible worker count for fff searches on the current machine.
fn search_threads() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
}

// A single grep-matched line within a file, with byte ranges for that line.
#[derive(Clone)]
pub struct GrepMatchLine {
    pub line_number: u64,
    pub line_content: String,
    pub byte_ranges: Vec<(u32, u32)>,
}

// A file path snapshot captured from a FileItem for render and preview work.
#[derive(Clone)]
pub struct FileItemSnapshot {
    pub file_name: String,
    pub dir: String,
    pub absolute_path: PathBuf,
    pub git_status: Option<String>,
    pub match_ranges: Vec<Range<usize>>,
    pub grep_matches: Vec<GrepMatchLine>,
}

pub struct FffPicker {
    shared_picker: SharedPicker,
    shared_frecency: SharedFrecency,
    shared_query_tracker: SharedQueryTracker,
    view: SearchView,
    grep_mode: GrepMode,
    query: String,
    results: Vec<FileItemSnapshot>,
    total_files: usize,
    total_matched: usize,
    selected: usize,
    selected_paths: BTreeSet<PathBuf>,
    scan_done: bool,
    search_epoch: u64,
    search_in_flight: bool,
    search_queued: bool,
    search_abort: Option<Arc<AtomicBool>>,
    preview_epoch: u64,
    preview_loading: bool,
    preview_loading_visible: bool,
    preview_scroll_row: usize,
    preview_start_line: usize,
    theme_version: u64,
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
    preview_scroll: UniformListScrollHandle,
    preview_lines: Vec<HighlightedLine>,
    status_message: Option<String>,
    text_field: Entity<TextField>,
    dismiss_on_blur: Option<Subscription>,
    dismiss_on_window_blur: Option<Subscription>,
}

// Find byte ranges where query characters appear in order.
fn find_match_ranges(query: &str, text: &str) -> Vec<Range<usize>> {
    let query = query.trim();
    if query.is_empty() {
        return vec![];
    }

    let fuzzy_chars: Vec<char> = query.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut run_start: Option<usize> = None;
    let mut run_end: usize = 0;

    for (byte_idx, orig_ch) in text.char_indices() {
        if qi >= fuzzy_chars.len() {
            break;
        }
        let ch_lower = orig_ch.to_lowercase().next().unwrap_or(orig_ch);
        if ch_lower == fuzzy_chars[qi] {
            if run_start.is_none() {
                run_start = Some(byte_idx);
            }
            run_end = byte_idx + orig_ch.len_utf8();
            qi += 1;
        } else if let Some(start) = run_start.take() {
            ranges.push(start..run_end);
        }
    }
    if let Some(start) = run_start {
        ranges.push(start..run_end);
    }

    if qi >= fuzzy_chars.len() {
        ranges
    } else {
        vec![]
    }
}

// Render text with character ranges highlighted in the match color.
fn render_highlighted(text: &str, ranges: &[Range<usize>]) -> Div {
    let palette = theme::palette();

    fn clamp_range_to_char_boundaries(
        text: &str,
        start: usize,
        end: usize,
    ) -> Option<Range<usize>> {
        let mut start = start.min(text.len());
        let mut end = end.min(text.len());

        while start > 0 && !text.is_char_boundary(start) {
            start -= 1;
        }
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }

        (start < end).then_some(start..end)
    }

    let mut ranges: Vec<Range<usize>> = ranges
        .iter()
        .filter_map(|range| clamp_range_to_char_boundaries(text, range.start, range.end))
        .collect();
    ranges.sort_by_key(|range| (range.start, range.end));

    if ranges.is_empty() {
        return div().flex().items_center().child(text.to_string());
    }

    let mut parts: Vec<Div> = Vec::new();
    let mut last = 0;

    for range in ranges {
        if range.start < last {
            continue;
        }
        if range.start > last {
            parts.push(
                div()
                    .text_color(rgb(palette.text_primary))
                    .child(text[last..range.start].to_string()),
            );
        }
        parts.push(
            div()
                .text_color(rgb(palette.match_highlight))
                .child(text[range.clone()].to_string()),
        );
        last = range.end;
    }
    if last < text.len() {
        parts.push(
            div()
                .text_color(rgb(palette.text_primary))
                .child(text[last..].to_string()),
        );
    }

    div().flex().items_center().children(parts)
}

// Shorten the directory segment shown in each result row.
fn shorten_dir_for_row(dir: &str, max_chars: usize) -> String {
    let trimmed = dir.trim_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }

    PathShortenStrategy::MiddleNumber.shorten_path(std::path::Path::new(trimmed), max_chars)
}

// Allow fuzzy fallback only for simple identifier-like queries.
//
// Code-shaped queries with spaces or punctuation should stay literal so
// `struct Data {` continues to search file contents instead of collapsing
// into a weak filename-only fuzzy match.
fn should_allow_fuzzy_fallback(query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() || query.chars().any(char::is_whitespace) {
        return false;
    }

    query
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

// Format a git status badge for a row.
fn git_status_badge(status: Option<&str>) -> Option<(&'static str, u32)> {
    match status {
        Some("modified") => Some(("M", 0xF5A524)),
        Some("staged_new") | Some("staged_modified") => Some(("A", 0x32D583)),
        Some("staged_deleted") | Some("deleted") => Some(("D", 0xF97066)),
        Some("renamed") => Some(("R", 0x8E8E93)),
        Some("untracked") => Some(("?", 0xA48EFF)),
        Some("ignored") => Some(("I", 0x6C6C70)),
        Some("clean") | None => None,
        Some(_) => Some(("?", 0x6C6C70)),
    }
}

// Run a live grep query using the upstream parser and grep engine.
fn execute_grep_search(
    picker: &FilePicker,
    query: &str,
    base: &PathBuf,
    abort_signal: Arc<AtomicBool>,
    grep_mode: GrepMode,
) -> (Vec<FileItemSnapshot>, usize, usize) {
    let parsed = fff_search::grep::parse_grep_query(query);
    let primary_mode = match grep_mode {
        GrepMode::PlainText => {
            if has_regex_metacharacters(query) {
                GrepMode::Regex
            } else {
                GrepMode::PlainText
            }
        }
        GrepMode::Regex => GrepMode::Regex,
        GrepMode::Fuzzy => GrepMode::Fuzzy,
    };

    let run = |mode| {
        picker.grep(
            &parsed,
            &GrepSearchOptions {
                mode,
                page_limit: 200,
                max_matches_per_file: 5,
                smart_case: true,
                abort_signal: Some(abort_signal.clone()),
                ..Default::default()
            },
        )
    };

    let mut grep_result = run(primary_mode);
    if grep_result.matches.is_empty() && primary_mode == GrepMode::PlainText {
        grep_result = run(GrepMode::Fuzzy);
    }

    let mut items: Vec<FileItemSnapshot> = Vec::new();
    let mut item_by_path = std::collections::HashMap::<PathBuf, usize>::new();
    for gm in &grep_result.matches {
        let Some(fi) = grep_result.files.get(gm.file_index) else {
            continue;
        };
        if fi.is_binary() {
            continue;
        }
        let absolute_path = fi.absolute_path(picker, base);
        let file_name = fi.file_name(picker);
        let dir = fi.dir_str(picker);
        let grep_match = GrepMatchLine {
            line_number: gm.line_number,
            line_content: gm.line_content.clone(),
            byte_ranges: gm.match_byte_offsets.iter().copied().collect(),
        };
        if let Some(&idx) = item_by_path.get(&absolute_path) {
            items[idx].grep_matches.push(grep_match);
        } else {
            item_by_path.insert(absolute_path.clone(), items.len());
            items.push(FileItemSnapshot {
                git_status: format_git_status_opt(fi.git_status).map(str::to_string),
                match_ranges: find_match_ranges(query, &file_name),
                file_name,
                dir,
                absolute_path,
                grep_matches: vec![grep_match],
            });
        }
    }

    let total_files_seen = grep_result.total_files.max(grep_result.filtered_file_count);
    let total_matched = items.len();
    (items, total_files_seen, total_matched)
}

impl FffPicker {
    // Create a new picker rooted at `base_path` and start the background file scan.
    pub fn new(
        base_path: PathBuf,
        shared: PickerSharedState,
        enable_content_indexing: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let text_field = cx.new(|cx| TextField::new("Search files...", cx));

        cx.observe(&text_field, |this, _entity, cx| {
            let new_query = this.text_field.read(cx).text();
            if new_query != this.query {
                this.query = new_query;
                this.status_message = None;
                this.selected_paths.clear();
                this.preview_scroll_row = 0;
                this.run_search(cx);
            }
        })
        .detach();

        let mut instance = Self {
            shared_picker: shared.shared_picker,
            shared_frecency: shared.shared_frecency,
            shared_query_tracker: shared.shared_query_tracker,
            view: SearchView::Files,
            grep_mode: GrepMode::Fuzzy,
            query: String::new(),
            results: Vec::new(),
            total_files: 0,
            total_matched: 0,
            selected: 0,
            selected_paths: BTreeSet::new(),
            scan_done: false,
            search_epoch: 0,
            search_in_flight: false,
            search_queued: false,
            search_abort: None,
            preview_epoch: 0,
            preview_loading: false,
            preview_loading_visible: false,
            preview_scroll_row: 0,
            preview_start_line: 1,
            theme_version: theme::version(),
            focus_handle: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            preview_scroll: UniformListScrollHandle::new(),
            preview_lines: Vec::new(),
            status_message: None,
            text_field,
            dismiss_on_blur: None,
            dismiss_on_window_blur: None,
        };

        instance.start_scan(base_path, enable_content_indexing, cx);
        instance
    }

    // Close the popup when the window loses focus, matching Raycast-style dismissal.
    pub fn install_focus_lost_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = self.focus_handle.clone();
        self.dismiss_on_blur =
            Some(
                cx.on_focus_out(&focus_handle, window, |_, _event, window, _cx| {
                    window.remove_window();
                }),
            );
        self.dismiss_on_window_blur = Some(cx.on_focus_lost(window, |_, window, _cx| {
            window.remove_window();
        }));
    }

    // Start the file indexer and trigger the initial search when indexing is ready.
    #[tracing::instrument(skip(self, cx, base_path), fields(base_path = %base_path.display(), enable_content_indexing))]
    fn start_scan(
        &mut self,
        base_path: PathBuf,
        enable_content_indexing: bool,
        cx: &mut Context<Self>,
    ) {
        let sp = self.shared_picker.clone();
        let sf = self.shared_frecency.clone();
        let sq = self.shared_query_tracker.clone();
        let existing_picker = self.shared_picker.read().ok().and_then(|guard| {
            guard.as_ref().map(|picker| {
                (
                    picker.base_path().to_path_buf(),
                    picker.is_scanning.load(Ordering::Acquire),
                )
            })
        });

        if let Some((existing_base_path, is_scanning)) = existing_picker
            && existing_base_path == base_path
        {
            info!(
                base_path = %base_path.display(),
                is_scanning,
                "reusing resident file index"
            );
            if is_scanning {
                cx.spawn(
                    async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                        let scan_done =
                            smol::unblock(move || sp.wait_for_scan(Duration::from_secs(60))).await;
                        if !scan_done {
                            warn!(base_path = %base_path.display(), "resident file scan timed out");
                        }

                        let update_result = this.update(cx, |this, cx| {
                            this.scan_done = true;
                            cx.notify();
                            this.run_search(cx);
                            info!(
                                scan_done = this.scan_done,
                                results = this.results.len(),
                                "resident scan state applied to picker"
                            );
                        });

                        if let Err(err) = update_result {
                            warn!(
                                error = %err,
                                "failed to apply resident scan state to picker"
                            );
                        }
                    },
                )
                .detach();
            } else {
                self.scan_done = true;
                self.run_search(cx);
                info!(
                    scan_done = self.scan_done,
                    results = self.results.len(),
                    "resident scan state applied to picker"
                );
            }
            return;
        }

        info!("starting file index");

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                smol::unblock(move || {
                    preview::warm_highlighter();

                    trace!(home = ?std::env::var("HOME").ok(), "initializing shared trackers");
                    if let Ok(home) = std::env::var("HOME") {
                        let data_dir = PathBuf::from(home).join(".local/share/fff");
                        let _ = std::fs::create_dir_all(&data_dir);
                        if let Ok(tracker) =
                            FrecencyTracker::new(data_dir.join("frecency.lmdb"), false)
                        {
                            let _ = sf.init(tracker);
                        }
                        if let Ok(tracker) = QueryTracker::new(
                            data_dir.join("queries.lmdb").to_string_lossy().as_ref(),
                            false,
                        ) {
                            let _ = sq.init(tracker);
                        }
                    }
                    if let Err(err) = FilePicker::new_with_shared_state(
                        sp.clone(),
                        sf,
                        FilePickerOptions {
                            base_path: base_path.to_string_lossy().to_string(),
                            enable_mmap_cache: false,
                            enable_content_indexing,
                            mode: FFFMode::Neovim,
                            watch: false,
                            ..Default::default()
                        },
                    ) {
                        error!(error = %err, base_path = %base_path.display(), "failed to initialize file picker");
                    }

                    let scan_completed = sp.wait_for_scan(Duration::from_secs(60));
                    if scan_completed {
                        info!(base_path = %base_path.display(), "initial file scan completed");
                    } else {
                        warn!(base_path = %base_path.display(), "initial file scan timed out");
                    }
                })
                .await;

                let update_result = this.update(cx, |this, cx| {
                    this.scan_done = true;
                    cx.notify();
                    this.run_search(cx);
                    info!(
                        scan_done = this.scan_done,
                        results = this.results.len(),
                        "scan completion applied to picker state"
                    );
                });

                if let Err(err) = update_result {
                    warn!(error = %err, "failed to apply scan completion to picker state");
                }
            },
        )
        .detach();
    }

    // Run the active search view and render the corresponding result set.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        if !self.scan_done {
            return;
        }

        if self.search_in_flight {
            self.search_epoch = self.search_epoch.wrapping_add(1);
            self.search_queued = true;
            if let Some(abort) = &self.search_abort {
                abort.store(true, Ordering::Release);
            }
            return;
        }

        self.search_epoch = self.search_epoch.wrapping_add(1);
        self.search_in_flight = true;
        let abort_signal = Arc::new(AtomicBool::new(false));
        self.search_abort = Some(abort_signal.clone());
        let epoch = self.search_epoch;
        let shared_picker = self.shared_picker.clone();
        let shared_query_tracker = self.shared_query_tracker.clone();
        let query_str = self.query.clone();
        let view = self.view;
        let grep_mode = self.grep_mode;
        debug!(
            epoch,
            query = %query_str.trim(),
            view = ?view,
            grep_mode = ?grep_mode,
            "starting search"
        );

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                let (items, total_files, total_matched) = smol::unblock(move || {
                    let Ok(guard) = shared_picker.read() else {
                        return (Vec::new(), 0, 0);
                    };
                    let Some(picker) = guard.as_ref() else {
                        return (Vec::new(), 0, 0);
                    };

                    let base = picker.base_path().to_path_buf();
                    let query = query_str.trim().to_string();

                    match view {
                        SearchView::Files => {
                            let parser = QueryParser::new(FileSearchConfig);
                            let parsed = parser.parse(&query);
                            let query_tracker = shared_query_tracker.read().ok();
                            let search = picker.fuzzy_search(
                                &parsed,
                                query_tracker
                                    .as_deref()
                                    .and_then(|tracker| tracker.as_ref()),
                                FuzzySearchOptions {
                                    max_threads: search_threads(),
                                    project_path: Some(picker.base_path()),
                                    combo_boost_score_multiplier: 100,
                                    min_combo_count: 3,
                                    pagination: PaginationArgs {
                                        offset: 0,
                                        limit: 200,
                                    },
                                    ..Default::default()
                                },
                            );
                            let fuzzy_items = search
                                .items
                                .iter()
                                .filter(|fi| !fi.is_binary())
                                .map(|fi| {
                                    let file_name = fi.file_name(picker);
                                    let dir = fi.dir_str(picker);
                                    let absolute_path = fi.absolute_path(picker, &base);
                                    FileItemSnapshot {
                                        git_status: format_git_status_opt(fi.git_status)
                                            .map(str::to_string),
                                        match_ranges: find_match_ranges(&query, &file_name),
                                        file_name,
                                        dir,
                                        absolute_path,
                                        grep_matches: vec![],
                                    }
                                })
                                .collect::<Vec<_>>();

                            if query.is_empty() {
                                return (fuzzy_items, search.total_files, search.total_matched);
                            }

                            let grep_query = fff_search::grep::parse_grep_query(&query);
                            let primary_mode = if has_regex_metacharacters(&query) {
                                GrepMode::Regex
                            } else {
                                GrepMode::PlainText
                            };

                            let run_grep = |mode| {
                                picker.grep(
                                    &grep_query,
                                    &GrepSearchOptions {
                                        mode,
                                        page_limit: 200,
                                        max_matches_per_file: 5,
                                        smart_case: true,
                                        abort_signal: Some(abort_signal.clone()),
                                        ..Default::default()
                                    },
                                )
                            };

                            let mut grep_result = run_grep(primary_mode);
                            if grep_result.matches.is_empty() && primary_mode == GrepMode::PlainText
                            {
                                grep_result = run_grep(GrepMode::Fuzzy);
                            }

                            let allow_fuzzy_fallback = should_allow_fuzzy_fallback(&query);
                            if !allow_fuzzy_fallback {
                                let mut items: Vec<FileItemSnapshot> = Vec::new();
                                let mut total_files_seen =
                                    grep_result.total_files.max(grep_result.filtered_file_count);

                                for gm in &grep_result.matches {
                                    let Some(fi) = grep_result.files.get(gm.file_index) else {
                                        continue;
                                    };
                                    if fi.is_binary() {
                                        continue;
                                    }
                                    let absolute_path = fi.absolute_path(picker, &base);
                                    let file_name = fi.file_name(picker);
                                    let dir = fi.dir_str(picker);
                                    items.push(FileItemSnapshot {
                                        git_status: format_git_status_opt(fi.git_status)
                                            .map(str::to_string),
                                        match_ranges: find_match_ranges(&query, &file_name),
                                        file_name,
                                        dir,
                                        absolute_path,
                                        grep_matches: vec![GrepMatchLine {
                                            line_number: gm.line_number,
                                            line_content: gm.line_content.clone(),
                                            byte_ranges: gm
                                                .match_byte_offsets
                                                .iter()
                                                .copied()
                                                .collect(),
                                        }],
                                    });
                                }

                                if items.is_empty() {
                                    total_files_seen = search.total_files;
                                }

                                let total_matched = items.len();
                                return (items, total_files_seen, total_matched);
                            }

                            let mut grep_by_path =
                                std::collections::HashMap::<PathBuf, Vec<GrepMatchLine>>::new();
                            let mut grep_order: Vec<PathBuf> = Vec::new();
                            let mut grep_fi_index =
                                std::collections::HashMap::<PathBuf, usize>::new();

                            for gm in &grep_result.matches {
                                let Some(fi) = grep_result.files.get(gm.file_index) else {
                                    continue;
                                };
                                if fi.is_binary() {
                                    continue;
                                }
                                let path = fi.absolute_path(picker, &base);
                                if !grep_by_path.contains_key(&path) {
                                    grep_order.push(path.clone());
                                    grep_fi_index.insert(path.clone(), gm.file_index);
                                }
                                grep_by_path.entry(path).or_default().push(GrepMatchLine {
                                    line_number: gm.line_number,
                                    line_content: gm.line_content.clone(),
                                    byte_ranges: gm.match_byte_offsets.iter().copied().collect(),
                                });
                            }

                            if grep_order.is_empty() {
                                return (fuzzy_items, search.total_files, search.total_matched);
                            }

                            let mut fuzzy_by_path: std::collections::HashMap<
                                PathBuf,
                                FileItemSnapshot,
                            > = std::collections::HashMap::new();
                            let mut fuzzy_order = Vec::with_capacity(fuzzy_items.len());
                            for item in fuzzy_items {
                                fuzzy_order.push(item.absolute_path.clone());
                                fuzzy_by_path.insert(item.absolute_path.clone(), item);
                            }

                            let mut merged: Vec<FileItemSnapshot> = Vec::new();

                            for path in &fuzzy_order {
                                let Some(mut item) = fuzzy_by_path.remove(path) else {
                                    continue;
                                };
                                if let Some(grep_matches) = grep_by_path.remove(path) {
                                    item.grep_matches = grep_matches;
                                }
                                merged.push(item);
                            }

                            for path in &grep_order {
                                let Some(grep_matches) = grep_by_path.remove(path) else {
                                    continue;
                                };
                                let Some(&fi_idx) = grep_fi_index.get(path) else {
                                    continue;
                                };
                                let Some(fi) = grep_result.files.get(fi_idx) else {
                                    continue;
                                };
                                let file_name = fi.file_name(picker);
                                let dir = fi.dir_str(picker);
                                merged.push(FileItemSnapshot {
                                    git_status: format_git_status_opt(fi.git_status)
                                        .map(str::to_string),
                                    match_ranges: find_match_ranges(&query, &file_name),
                                    file_name,
                                    dir,
                                    absolute_path: path.clone(),
                                    grep_matches,
                                });
                            }

                            let total_matched = merged.len().max(search.total_matched);
                            (merged, search.total_files, total_matched)
                        }
                        SearchView::Grep => {
                            if query.is_empty() {
                                return (Vec::new(), 0, 0);
                            }

                            execute_grep_search(picker, &query_str, &base, abort_signal, grep_mode)
                        }
                    }
                })
                .await;

                let update_result = this.update(cx, |this, cx| {
                    if this.search_epoch != epoch {
                        trace!(epoch, "discarding stale search result");
                        this.finish_search(cx);
                        return;
                    }
                    debug!(
                        epoch,
                        results = items.len(),
                        total_files,
                        total_matched,
                        "applying search result"
                    );
                    this.results = items;
                    this.total_files = total_files;
                    this.total_matched = total_matched;
                    this.selected = 0;
                    this.preview_scroll_row = 0;
                    this.selected_paths
                        .retain(|path| this.results.iter().any(|item| &item.absolute_path == path));
                    this.load_preview(cx);
                    cx.notify();
                    this.finish_search(cx);
                    trace!(
                        epoch,
                        scan_done = this.scan_done,
                        results = this.results.len(),
                        "search result applied to picker state"
                    );
                });

                if let Err(err) = update_result {
                    warn!(error = %err, epoch, "failed to apply search result to picker state");
                }
            },
        )
        .detach();
    }

    // Finish the active search and schedule any query that arrived while it was running.
    fn finish_search(&mut self, cx: &mut Context<Self>) {
        self.search_in_flight = false;
        self.search_abort = None;
        if self.search_queued {
            self.search_queued = false;
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                this.update(cx, |this, cx| {
                    this.run_search(cx);
                })
                .ok();
            });
        }
    }

    // Load and syntax-highlight the selected file preview in the background.
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_epoch = self.preview_epoch.wrapping_add(1);
        let preview_epoch = self.preview_epoch;
        let (path, grep_matches) = match self.results.get(self.selected) {
            Some(r) => (r.absolute_path.clone(), r.grep_matches.clone()),
            None => {
                self.preview_lines = vec![];
                self.preview_loading = false;
                self.preview_loading_visible = false;
                self.preview_scroll_row = 0;
                self.preview_start_line = 1;
                return;
            }
        };

        self.preview_loading = true;
        self.preview_loading_visible = false;
        trace!(
            preview_epoch,
            path = %path.display(),
            grep_matches = grep_matches.len(),
            "loading preview"
        );
        let first_match_line = grep_matches.iter().map(|m| m.line_number as usize).min();
        let match_highlight = theme::palette().match_highlight;

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                smol::Timer::after(Duration::from_millis(120)).await;
                this.update(cx, |this, cx| {
                    if this.preview_epoch == preview_epoch
                        && this.preview_loading
                        && this.preview_lines.is_empty()
                    {
                        this.preview_loading_visible = true;
                        cx.notify();
                    }
                })
                .ok();
            },
        )
        .detach();

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                let (start_line, lines) = smol::unblock(move || {
                    let (start_line, mut lines) =
                        preview::highlight_file_window(&path, first_match_line);
                    for gm in &grep_matches {
                        let idx = (gm.line_number as usize).saturating_sub(start_line);
                        if let Some(line) = lines.get_mut(idx) {
                            line.spans = preview::overlay_match_ranges(
                                &line.spans,
                                &gm.byte_ranges,
                                match_highlight,
                            );
                        }
                    }
                    (start_line, lines)
                })
                .await;

                this.update(cx, |this, cx| {
                    if this.preview_epoch != preview_epoch {
                        trace!(preview_epoch, "discarding stale preview result");
                        return;
                    }
                    this.preview_lines = lines;
                    this.preview_loading = false;
                    this.preview_loading_visible = false;
                    this.preview_start_line = start_line;
                    this.preview_scroll_row = first_match_line
                        .map(|line| line.saturating_sub(start_line))
                        .unwrap_or(0);
                    let scroll_to = this.preview_scroll_row;
                    this.preview_scroll.scroll_to_item(
                        scroll_to.saturating_sub(preview::MATCH_CONTEXT_BEFORE),
                        ScrollStrategy::Top,
                    );
                    cx.notify();
                })
                .ok();
            },
        )
        .detach();
    }

    // Open the selected file or the first selected file and track the query.
    fn on_open_selected(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        let selected_path = self.selected_paths.iter().next().cloned().or_else(|| {
            self.results
                .get(self.selected)
                .map(|item| item.absolute_path.clone())
        });
        let Some(path) = selected_path else {
            return;
        };

        if let Ok(guard) = self.shared_picker.read()
            && let Some(picker) = guard.as_ref()
        {
            if let Ok(mut tracker_guard) = self.shared_query_tracker.write()
                && let Some(tracker) = tracker_guard.as_mut()
            {
                let project_path = picker.base_path();
                match self.view {
                    SearchView::Files => {
                        let _ = tracker.track_query_completion(&self.query, project_path, &path);
                    }
                    SearchView::Grep => {
                        let _ = tracker.track_grep_query(&self.query, project_path);
                    }
                }
            }
        }

        if let Ok(guard) = self.shared_frecency.read()
            && let Some(tracker) = guard.as_ref()
        {
            let _ = tracker.track_access(&path);
        }

        match editor::open_in_editor(&path, None) {
            Ok(child) => {
                info!(pid = child.id(), path = %path.display(), "spawned editor");
                self.status_message = Some(format!("Opened {}", path.display()));
                cx.notify();
                window.remove_window();
            }
            Err(err) => {
                error!(error = %err, path = %path.display(), "open failed");
                let message = format!("Open failed: {err}");
                self.status_message =
                    Some(format!("{message}  (log: {})", log::path_for_display()));
                cx.notify();
            }
        }
    }

    // Close the current picker window without terminating the resident service.
    fn on_quit(&mut self, _: &Quit, window: &mut Window, _cx: &mut Context<Self>) {
        window.remove_window();
    }

    // Move selection down one row.
    fn on_select_next(&mut self, _: &SelectNext, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.results.is_empty() {
            self.selected = (self.selected + 1).min(self.results.len() - 1);
            self.list_scroll
                .scroll_to_item(self.selected, ScrollStrategy::Center);
            self.load_preview(cx);
            cx.notify();
        }
    }

    // Move selection up one row.
    fn on_select_prev(&mut self, _: &SelectPrev, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected > 0 {
            self.selected -= 1;
            self.list_scroll
                .scroll_to_item(self.selected, ScrollStrategy::Center);
            self.load_preview(cx);
            cx.notify();
        }
    }

    // Select the clicked row and refresh the preview.
    fn on_select_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.results.len() {
            return;
        }

        if self.selected == index {
            self.on_open_selected(&OpenSelected, window, cx);
            return;
        }

        self.selected = index;
        self.list_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Center);
        self.load_preview(cx);
        window.focus(&self.text_field_focus_handle(cx));
        cx.notify();
    }

    // Toggle the selected state for the current row.
    fn on_toggle_selected(
        &mut self,
        _: &ToggleSelected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.results.get(self.selected) else {
            return;
        };
        if !self.selected_paths.insert(item.absolute_path.clone()) {
            self.selected_paths.remove(&item.absolute_path);
        }
        cx.notify();
    }

    // Cycle through the available grep modes.
    fn on_cycle_grep_mode(
        &mut self,
        _: &CycleGrepMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.grep_mode = match self.grep_mode {
            GrepMode::PlainText => GrepMode::Regex,
            GrepMode::Regex => GrepMode::Fuzzy,
            GrepMode::Fuzzy => GrepMode::PlainText,
        };
        self.status_message = Some(format!("Grep mode: {:?}", self.grep_mode));
        self.run_search(cx);
    }

    // Restore the previous query from the local search history.
    fn on_cycle_previous_query(
        &mut self,
        _: &CyclePreviousQuery,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(query) = (|| {
            let guard = self.shared_query_tracker.read().ok()?;
            let tracker = guard.as_ref()?;
            let picker_guard = self.shared_picker.read().ok()?;
            let picker = picker_guard.as_ref()?;
            let project_path = picker.base_path();
            match self.view {
                SearchView::Files => tracker.get_historical_query(project_path, 0).ok().flatten(),
                SearchView::Grep => tracker
                    .get_historical_grep_query(project_path, 0)
                    .ok()
                    .flatten(),
            }
        })() else {
            self.status_message = Some("No query history".to_string());
            cx.notify();
            return;
        };

        self.text_field
            .update(cx, |field, cx| field.set_text(query, cx));
    }

    // Scroll the preview pane toward the top.
    fn on_preview_scroll_up(
        &mut self,
        _: &PreviewScrollUp,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_scroll_row = self.preview_scroll_row.saturating_sub(6);
        self.preview_scroll
            .scroll_to_item(self.preview_scroll_row, ScrollStrategy::Top);
        cx.notify();
    }

    // Scroll the preview pane toward the bottom.
    fn on_preview_scroll_down(
        &mut self,
        _: &PreviewScrollDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.preview_lines.is_empty() {
            self.preview_scroll_row =
                (self.preview_scroll_row + 6).min(self.preview_lines.len() - 1);
            self.preview_scroll
                .scroll_to_item(self.preview_scroll_row, ScrollStrategy::Top);
            cx.notify();
        }
    }

    // Switch back to file search mode.
    fn on_switch_files(&mut self, _: &SwitchFiles, _window: &mut Window, cx: &mut Context<Self>) {
        if self.view != SearchView::Files {
            self.view = SearchView::Files;
            self.status_message = None;
            self.run_search(cx);
        }
    }

    // Switch to live grep mode.
    fn on_switch_grep(&mut self, _: &SwitchGrep, _window: &mut Window, cx: &mut Context<Self>) {
        if self.view != SearchView::Grep {
            self.view = SearchView::Grep;
            self.status_message = None;
            self.run_search(cx);
        }
    }

    // Return the text field focus handle so the window can focus it on startup.
    pub fn text_field_focus_handle(&self, cx: &App) -> FocusHandle {
        self.text_field.focus_handle(cx)
    }
}

impl Render for FffPicker {
    // Render the picker layout.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_theme_version = theme::version();
        if self.theme_version != current_theme_version {
            self.theme_version = current_theme_version;
            if self.selected < self.results.len() {
                self.load_preview(cx);
            }
        }
        let palette = theme::palette();
        let ui_font_family = theme::ui_font_family();
        let buffer_font_family = theme::buffer_font_family();
        let preview_line_height = px(14.0);
        let results = self.results.clone();
        let preview_lines = self.preview_lines.clone();
        let selected = self.selected;
        let scan_done = self.scan_done;
        let total_files = self.total_files;
        let total_matched = self.total_matched;
        let selected_count = self.selected_paths.len();
        let selected_paths = self.selected_paths.clone();
        let list_scroll = self.list_scroll.clone();
        let preview_scroll = self.preview_scroll.clone();
        let selected_path = results.get(selected).map(|item| item.absolute_path.clone());
        trace!(
            scan_done,
            results = results.len(),
            selected,
            preview_lines = preview_lines.len(),
            selected_count,
            view = ?self.view,
            query = %self.query,
            status_message = ?self.status_message,
            "rendering picker"
        );
        let preview_placeholder = if !scan_done {
            ""
        } else if self.preview_loading_visible {
            "Loading\u{2026}"
        } else if selected_path.is_some() && preview_lines.is_empty() {
            "No preview"
        } else if self.view == SearchView::Grep && self.query.trim().is_empty() {
            "Type to grep"
        } else {
            "No preview"
        };

        let status_text = if let Some(message) = self.status_message.clone() {
            message
        } else if !scan_done {
            String::new()
        } else {
            format!(
                "{} shown  {selected_count} selected  {total_matched} matches  {total_files} indexed",
                results.len()
            )
        };

        div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_open_selected))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_toggle_selected))
            .on_action(cx.listener(Self::on_cycle_grep_mode))
            .on_action(cx.listener(Self::on_cycle_previous_query))
            .on_action(cx.listener(Self::on_preview_scroll_up))
            .on_action(cx.listener(Self::on_preview_scroll_down))
            .on_action(cx.listener(Self::on_switch_files))
            .on_action(cx.listener(Self::on_switch_grep))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text_primary))
            .font_family(ui_font_family.clone())
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(
                        div()
                            .w(px(430.0))
                            .h_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                    .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .when(!scan_done, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(palette.text_dim))
                                .child("Indexing\u{2026}"),
                        )
                    })
                    .when(scan_done && results.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_1()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(palette.text_dim))
                                .child("No files matched"),
                        )
                    })
                    .when(scan_done && !results.is_empty(), |this| {
                        let list_panel = div()
                            .w_full()
                            .h_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                uniform_list(
                                    "results",
                                    results.len(),
                                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                                        range
                                            .map(|i| {
                                                let item = &results[i];
                                                let is_selected = i == selected;
                                                let is_marked = selected_paths.contains(&item.absolute_path);
                                                let display_dir = shorten_dir_for_row(&item.dir, 30);
                                                let git_badge =
                                                    git_status_badge(item.git_status.as_deref());

                                                let content_match: Option<(String, Vec<Range<usize>>)> =
                                                    if item.match_ranges.is_empty() {
                                                        item.grep_matches.first().map(|m| {
                                                            let strip = m.line_content.len()
                                                                - m.line_content.trim_start().len();
                                                            let text =
                                                                m.line_content.trim().to_string();
                                                            let ranges = m
                                                                .byte_ranges
                                                                .iter()
                                                                .filter_map(|&(s, e)| {
                                                                    let s = (s as usize)
                                                                        .saturating_sub(strip);
                                                                    let e = (e as usize)
                                                                        .saturating_sub(strip)
                                                                        .min(text.len());
                                                                    if s < e { Some(s..e) } else { None }
                                                                })
                                                                .collect();
                                                            (text, ranges)
                                                        })
                                                    } else {
                                                        None
                                                    };

                                                div()
                                                    .id(("row", i))
                                                    .w_full()
                                                    .h(px(28.0))
                                                    .pl(px(10.0))
                                                    .pr(px(12.0))
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(8.0))
                                                    .bg(if is_selected {
                                                        rgb(palette.selected_row)
                                                    } else {
                                                        rgb(palette.bg)
                                                    })
                                                    .hover(|s| s.bg(rgb(palette.hover_row)))
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        this.on_select_row(i, window, cx);
                                                    }))
                                                    .child(
                                                        div()
                                                            .w(px(8.0))
                                                            .flex_shrink_0()
                                                            .text_color(if is_selected {
                                                                rgb(palette.match_highlight)
                                                            } else if is_marked {
                                                                rgb(palette.text_primary)
                                                            } else {
                                                                rgb(palette.text_dim)
                                                            })
                                                            .child(if is_marked {
                                                                "\u{258A}"
                                                            } else if is_selected {
                                                                "\u{203A}"
                                                            } else {
                                                                " "
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .overflow_hidden()
                                                            .text_sm()
                                                            .when(content_match.is_some(), |d| {
                                                                let (text, ranges) =
                                                                    content_match.as_ref().unwrap();
                                                                d.flex()
                                                                    .items_center()
                                                                    .gap(px(6.0))
                                                                    .child(
                                                                        div()
                                                                            .text_color(rgb(
                                                                                palette.text_dim,
                                                                            ))
                                                                            .flex_shrink_0()
                                                                            .child(
                                                                                item.file_name
                                                                                    .clone(),
                                                                            ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_color(rgb(
                                                                                palette.text_secondary,
                                                                            ))
                                                                            .flex_shrink_0()
                                                                            .child(format!(
                                                                                ":{}",
                                                                                item.grep_matches
                                                                                    .first()
                                                                                    .map(|m| m.line_number)
                                                                                    .unwrap_or(0)
                                                                            )),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .min_w(px(0.0))
                                                                            .overflow_hidden()
                                                                            .child(
                                                                                render_highlighted(
                                                                                    text, ranges,
                                                                                ),
                                                                            ),
                                                                    )
                                                            })
                                                            .when(content_match.is_none(), |d| {
                                                                d.child(render_highlighted(
                                                                    &item.file_name,
                                                                    &item.match_ranges,
                                                                ))
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_shrink_0()
                                                            .items_center()
                                                            .gap(px(4.0))
                                                            .when(git_badge.is_some(), |this| {
                                                                let (label, color) = git_badge.unwrap();
                                                                this.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(color))
                                                                        .child(label),
                                                                )
                                                            })
                                                            .when(
                                                                !item.grep_matches.is_empty(),
                                                                |this| {
                                                                    this.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                palette.match_highlight,
                                                                            ))
                                                                            .flex_shrink_0()
                                                                            .child(format!(
                                                                                "{}",
                                                                                item.grep_matches
                                                                                    .len()
                                                                            )),
                                                                    )
                                                                },
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(
                                                                        palette.text_secondary,
                                                                    ))
                                                                    .max_w(px(190.0))
                                                                    .flex_shrink_0()
                                                                    .overflow_hidden()
                                                                    .child(display_dir),
                                                            ),
                                                    )
                                            })
                                            .collect()
                                    }),
                                )
                                .flex_1()
                                .w_full()
                                .track_scroll(list_scroll),
                            );
                        this.child(list_panel)
                    }),
                    )
                    .child(
                div()
                    .w_full()
                    .h(px(46.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .text_color(rgb(palette.match_highlight))
                            .text_sm()
                            .child("🪿"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .min_w(px(0.0))
                            .font_family(buffer_font_family.clone())
                            .child(self.text_field.clone()),
                    ),
                    )
            )
            .child(
                div()
                    .w(px(1.0))
                    .h_full()
                    .bg(rgb(palette.border))
                    .flex_shrink_0(),
            )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(rgb(palette.preview_bg))
                            .overflow_hidden()
                            .when(preview_lines.is_empty(), |this| {
                                this.child(
                                    div()
                                        .size_full()
                                        .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(palette.text_dim))
                                .child(preview_placeholder),
                        )
                    })
                    .when(!preview_lines.is_empty(), |this| {
                        this.child(
                            uniform_list("preview", preview_lines.len(), move |range, _window, _cx| {
                                range
                                    .map(|i| {
                                        let line = &preview_lines[i];
                                        div()
                                            .id(("pl", i))
                                            .h(preview_line_height)
                                            .px(px(8.0))
                                            .flex()
                                            .items_center()
                                            .font_family(buffer_font_family.clone())
                                            .children(line.spans.iter().map(|span| {
                                                div()
                                                    .text_xs()
                                                    .line_height(px(14.0))
                                                    .text_color(rgb(span.color))
                                                    .when(span.bold, |d| d.font_weight(FontWeight::BOLD))
                                                    .when(span.italic, |d| d.italic())
                                                    .when(span.underline, |d| d.underline())
                                                    .when(span.strikethrough, |d| d.line_through())
                                                    .child(span.text.clone())
                                            }))
                                    })
                                    .collect()
                            })
                            .flex_1()
                            .w_full()
                            .track_scroll(preview_scroll),
                        )
                    }),
            )
            )
            .child(
                div()
                    .w_full()
                    .h(px(28.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(palette.status_bar_bg))
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_dim))
                            .child(status_text),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_dim))
                            .child("\u{2191}\u{2193} navigate  \u{23CE} open  esc quit"),
                    ),
            )
    }
}
