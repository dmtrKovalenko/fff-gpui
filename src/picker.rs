use std::ops::Range;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use fff_search::{
    FFFMode, FilePickerOptions, FileSearchConfig, FuzzySearchOptions, GrepMode, GrepSearchOptions,
    PaginationArgs, QueryParser, SharedFrecency, SharedPicker, SharedQueryTracker,
    file_picker::FilePicker, frecency::FrecencyTracker, grep::parse_grep_query,
    query_tracker::QueryTracker,
};
use gpui::prelude::*;
use gpui::*;

use crate::editor;
use crate::log;
use crate::path_shortening::PathShortenStrategy;
use crate::preview::{self, HighlightedLine};
use crate::text_field::TextField;
use crate::theme;

actions!(fff_picker, [Quit, OpenSelected, SelectNext, SelectPrev]);

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
    pub match_ranges: Vec<Range<usize>>,
    pub grep_matches: Vec<GrepMatchLine>,
}

pub struct FffPicker {
    shared_picker: SharedPicker,
    shared_frecency: SharedFrecency,
    shared_query_tracker: SharedQueryTracker,
    query: String,
    results: Vec<FileItemSnapshot>,
    total_files: usize,
    total_matched: usize,
    selected: usize,
    scan_done: bool,
    search_epoch: u64,
    search_in_flight: bool,
    search_queued: bool,
    search_abort: Option<Arc<AtomicBool>>,
    preview_epoch: u64,
    focus_handle: FocusHandle,
    list_scroll: UniformListScrollHandle,
    preview_scroll: UniformListScrollHandle,
    preview_lines: Vec<HighlightedLine>,
    status_message: Option<String>,
    text_field: Entity<TextField>,
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
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(text[last..range.start].to_string()),
            );
        }
        parts.push(
            div()
                .text_color(rgb(theme::MATCH_HIGHLIGHT))
                .child(text[range.clone()].to_string()),
        );
        last = range.end;
    }
    if last < text.len() {
        parts.push(
            div()
                .text_color(rgb(theme::TEXT_PRIMARY))
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
// `struct Data {` doesn't degrade into a partial match on `struct`.
fn should_allow_fuzzy_fallback(query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() || query.chars().any(char::is_whitespace) {
        return false;
    }

    query
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl FffPicker {
    // Create a new picker rooted at `base_path` and start the background file scan.
    pub fn new(base_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let text_field = cx.new(|cx| TextField::new("Search files...", cx));

        cx.observe(&text_field, |this, _entity, cx| {
            let new_query = this.text_field.read(cx).text();
            if new_query != this.query {
                this.query = new_query;
                this.run_search(cx);
            }
        })
        .detach();

        let mut instance = Self {
            shared_picker: SharedPicker::default(),
            shared_frecency: SharedFrecency::default(),
            shared_query_tracker: SharedQueryTracker::default(),
            query: String::new(),
            results: Vec::new(),
            total_files: 0,
            total_matched: 0,
            selected: 0,
            scan_done: false,
            search_epoch: 0,
            search_in_flight: false,
            search_queued: false,
            search_abort: None,
            preview_epoch: 0,
            focus_handle: cx.focus_handle(),
            list_scroll: UniformListScrollHandle::new(),
            preview_scroll: UniformListScrollHandle::new(),
            preview_lines: Vec::new(),
            status_message: None,
            text_field,
        };

        instance.start_scan(base_path, cx);
        instance
    }

    // Start the file indexer and trigger the initial search when indexing is ready.
    fn start_scan(&mut self, base_path: PathBuf, cx: &mut Context<Self>) {
        let sp = self.shared_picker.clone();
        let sf = self.shared_frecency.clone();
        let sq = self.shared_query_tracker.clone();

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                smol::unblock(move || {
                    preview::warm_highlighter();

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
                    let _ = FilePicker::new_with_shared_state(
                        sp.clone(),
                        sf,
                        FilePickerOptions {
                            base_path: base_path.to_string_lossy().to_string(),
                            enable_mmap_cache: true,
                            enable_content_indexing: true,
                            mode: FFFMode::Neovim,
                            watch: false,
                            ..Default::default()
                        },
                    );
                    sp.wait_for_scan(Duration::from_secs(60));
                })
                .await;

                this.update(cx, |this, cx| {
                    this.scan_done = true;
                    cx.notify();
                    this.run_search(cx);
                })
                .ok();
            },
        )
        .detach();
    }

    // Run path and content search together, then render one merged result set.
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

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                let (items, total_files, total_matched) = {
                    let sp = shared_picker.clone();
                    let sq = shared_query_tracker.clone();
                    let q = query_str.clone();
                    smol::unblock(move || -> (Vec<FileItemSnapshot>, usize, usize) {
                        let Ok(guard) = sp.read() else {
                            return (vec![], 0, 0);
                        };
                        let Some(picker) = guard.as_ref() else {
                            return (vec![], 0, 0);
                        };

                        let parser = QueryParser::new(FileSearchConfig);
                        let query = parser.parse(&q);
                        let base = picker.base_path().to_path_buf();
                        let query_tracker = sq.read().ok();
                        let search = picker.fuzzy_search(
                            &query,
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
                        let total_files = search.total_files;
                        let fuzzy_total_matched = search.total_matched;
                        let fuzzy_items: Vec<FileItemSnapshot> = search
                            .items
                            .iter()
                            .filter(|fi| !fi.is_binary())
                            .map(|fi| {
                                let file_name = fi.file_name(picker);
                                let dir = fi.dir_str(picker);
                                let absolute_path = fi.absolute_path(picker, &base);
                                let match_ranges = find_match_ranges(&q, &file_name);
                                FileItemSnapshot {
                                    file_name,
                                    dir,
                                    absolute_path,
                                    match_ranges,
                                    grep_matches: vec![],
                                }
                            })
                            .collect();

                        if q.trim().is_empty() {
                            return (fuzzy_items, total_files, fuzzy_total_matched);
                        }

                        let query = parse_grep_query(&q);
                        let content_only = !should_allow_fuzzy_fallback(&q);
                        let grep_result = picker.grep(
                            &query,
                            &GrepSearchOptions {
                                mode: GrepMode::PlainText,
                                page_limit: 200,
                                max_matches_per_file: 5,
                                smart_case: true,
                                abort_signal: Some(abort_signal),
                                ..Default::default()
                            },
                        );

                        // Index grep matches by path (not file index) so we can join with fuzzy
                        // results using path as the key.
                        let mut grep_by_path: std::collections::HashMap<
                            PathBuf,
                            Vec<GrepMatchLine>,
                        > = std::collections::HashMap::new();
                        // grep_order preserves the ranking returned by the grep engine.
                        let mut grep_order: Vec<PathBuf> = Vec::new();
                        // Keep one fi_index per path so we can reconstruct FileItemSnapshot for
                        // grep-only hits later.
                        let mut grep_fi_index: std::collections::HashMap<PathBuf, usize> =
                            std::collections::HashMap::new();
                        for gm in grep_result.matches.iter() {
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
                            grep_by_path
                                .entry(path)
                                .or_default()
                                .push(GrepMatchLine {
                                    line_number: gm.line_number,
                                    line_content: gm.line_content.clone(),
                                    byte_ranges: gm.match_byte_offsets.iter().copied().collect(),
                                });
                        }

                        // content_only (query has spaces / punctuation): show only grep hits.
                        if content_only {
                            if grep_order.is_empty() {
                                return (vec![], total_files, 0);
                            }
                            let mut items: Vec<FileItemSnapshot> = Vec::new();
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
                                items.push(FileItemSnapshot {
                                    match_ranges: find_match_ranges(&q, &file_name),
                                    file_name,
                                    dir,
                                    absolute_path: path.clone(),
                                    grep_matches,
                                });
                            }
                            let total_matched = items.len();
                            return (items, total_files, total_matched);
                        }

                        // Identifier-like query: fuzzy (filename) results rank first so that
                        // e.g. "main.rs" surfaces the actual file above files that reference it.
                        // Grep matches are attached to fuzzy hits when available, and any
                        // content-only hits are appended afterwards.
                        if grep_order.is_empty() {
                            return (fuzzy_items, total_files, fuzzy_total_matched);
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

                        let mut items: Vec<FileItemSnapshot> = Vec::new();

                        // 1. Fuzzy results in score order; attach grep matches where available.
                        for path in &fuzzy_order {
                            let Some(mut item) = fuzzy_by_path.remove(path) else {
                                continue;
                            };
                            if let Some(grep_matches) = grep_by_path.remove(path) {
                                item.grep_matches = grep_matches;
                            }
                            items.push(item);
                        }

                        // 2. Content-only hits (grep results not present in fuzzy results).
                        for path in &grep_order {
                            let Some(grep_matches) = grep_by_path.remove(path) else {
                                continue; // already consumed above
                            };
                            let Some(&fi_idx) = grep_fi_index.get(path) else {
                                continue;
                            };
                            let Some(fi) = grep_result.files.get(fi_idx) else {
                                continue;
                            };
                            let file_name = fi.file_name(picker);
                            let dir = fi.dir_str(picker);
                            items.push(FileItemSnapshot {
                                match_ranges: find_match_ranges(&q, &file_name),
                                file_name,
                                dir,
                                absolute_path: path.clone(),
                                grep_matches,
                            });
                        }

                        let total_matched = items.len().max(fuzzy_total_matched);
                        (items, total_files, total_matched)
                    })
                    .await
                };

                this.update(cx, |this, cx| {
                    if this.search_epoch != epoch {
                        this.finish_search(cx);
                        return;
                    }
                    this.results = items;
                    this.total_files = total_files;
                    this.total_matched = total_matched;
                    this.selected = 0;
                    this.load_preview(cx);
                    cx.notify();
                    this.finish_search(cx);
                })
                .ok();
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
                return;
            }
        };

        let first_match_line = grep_matches.iter().map(|m| m.line_number as usize).min();

        cx.spawn(
            async move |this: WeakEntity<FffPicker>, cx: &mut AsyncApp| {
                let (start_line, lines) = smol::unblock(move || {
                    let (start_line, mut lines) =
                        preview::highlight_file_window(&path, first_match_line);
                    for gm in &grep_matches {
                        let idx = (gm.line_number as usize).saturating_sub(start_line);
                        if let Some(line) = lines.get_mut(idx) {
                            line.spans =
                                preview::overlay_match_ranges(&line.spans, &gm.byte_ranges);
                        }
                    }
                    (start_line, lines)
                })
                .await;

                this.update(cx, |this, cx| {
                    if this.preview_epoch != preview_epoch {
                        return;
                    }
                    this.preview_lines = lines;
                    let scroll_to = first_match_line
                        .map(|line| line.saturating_sub(start_line))
                        .unwrap_or(0);
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

    // Quit the picker process.
    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    // Open the selected file and leave the finder window visible for the next invocation.
    fn on_open_selected(&mut self, _: &OpenSelected, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.results.get(self.selected) else {
            return;
        };
        let path = item.absolute_path.clone();

        if let Ok(guard) = self.shared_frecency.read() {
            if let Some(tracker) = guard.as_ref() {
                let _ = tracker.track_access(&path);
            }
        }

        match editor::open_in_editor(&path) {
            Ok(child) => {
                log::append(format!(
                    "fff-gpui: spawned editor pid {} for {}",
                    child.id(),
                    path.display()
                ));
                self.status_message = Some(format!("Opened {}", path.display()));
                cx.notify();
            }
            Err(err) => {
                let message = format!("Open failed: {err}");
                log::append(format!("fff-gpui: {message}"));
                self.status_message =
                    Some(format!("{message}  (log: {})", log::path_for_display()));
                cx.notify();
            }
        }
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

    // Return the text field focus handle so the window can focus it on startup.
    pub fn text_field_focus_handle(&self, cx: &App) -> FocusHandle {
        self.text_field.focus_handle(cx)
    }
}

impl Render for FffPicker {
    // Render the picker layout.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let results = self.results.clone();
        let preview_lines = self.preview_lines.clone();
        let selected = self.selected;
        let scan_done = self.scan_done;
        let total_files = self.total_files;
        let total_matched = self.total_matched;
        let list_scroll = self.list_scroll.clone();
        let preview_scroll = self.preview_scroll.clone();
        let selected_path = results.get(selected).map(|item| item.absolute_path.clone());
        let preview_placeholder = if selected_path.is_some() {
            "Loading\u{2026}"
        } else {
            "No preview"
        };

        let status_text = if let Some(message) = self.status_message.clone() {
            message
        } else if !scan_done {
            "Indexing\u{2026}".to_string()
        } else {
            format!(
                "{} shown  {total_matched} file matches  {total_files} indexed",
                results.len()
            )
        };

        div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_open_selected))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG))
            .text_color(white())
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
                            .w_full()
                            .h(px(30.0))
                            .pl(px(80.0))
                            .pr(px(12.0))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(rgb(theme::BORDER))
                            .text_xs()
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child("fff-gpui"),
                    )
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
                                .text_color(rgb(theme::TEXT_DIM))
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
                                .text_color(rgb(theme::TEXT_DIM))
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
                                    move |range, _window, _cx| {
                                        range
                                            .map(|i| {
                                                let item = &results[i];
                                                let is_selected = i == selected;
                                                let display_dir = shorten_dir_for_row(&item.dir, 30);

                                                // For content-only matches show the first matched
                                                // line instead of the filename (which won't match).
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
                                                        rgb(theme::SELECTED_ROW)
                                                    } else {
                                                        rgb(theme::BG)
                                                    })
                                                    .hover(|s| s.bg(rgb(theme::HOVER_ROW)))
                                                    .child(
                                                        div()
                                                            .w(px(8.0))
                                                            .flex_shrink_0()
                                                            .text_color(if is_selected {
                                                                rgb(theme::MATCH_HIGHLIGHT)
                                                            } else {
                                                                rgb(theme::TEXT_DIM)
                                                            })
                                                            .child(if is_selected { "\u{203A}" } else { " " }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .overflow_hidden()
                                                            .text_sm()
                                                            .when(content_match.is_some(), |d| {
                                                                // Show filename dim + matched content
                                                                let (text, ranges) =
                                                                    content_match.as_ref().unwrap();
                                                                d.flex()
                                                                    .items_center()
                                                                    .gap(px(6.0))
                                                                    .child(
                                                                        div()
                                                                            .text_color(rgb(
                                                                                theme::TEXT_DIM,
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
                                                                                theme::TEXT_SECONDARY,
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
                                                            .when(
                                                                !item.grep_matches.is_empty(),
                                                                |this| {
                                                                    this.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                theme::MATCH_HIGHLIGHT,
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
                                                                        theme::TEXT_SECONDARY,
                                                                    ))
                                                                    .max_w(px(190.0))
                                                                    .flex_shrink_0()
                                                                    .overflow_hidden()
                                                                    .child(display_dir),
                                                            ),
                                                    )
                                            })
                                            .collect()
                                    },
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
                    .border_color(rgb(theme::BORDER))
                    .child(
                        div()
                            .text_color(rgb(theme::MATCH_HIGHLIGHT))
                            .text_sm()
                            .child("fff"),
                    )
                    .child(self.text_field.clone()),
                    )
            )
            .child(
                div()
                    .w(px(1.0))
                    .h_full()
                    .bg(rgb(theme::BORDER))
                    .flex_shrink_0(),
            )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(rgb(theme::PREVIEW_BG))
                            .overflow_hidden()
                            .when(preview_lines.is_empty(), |this| {
                                this.child(
                                    div()
                                        .size_full()
                                        .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(theme::TEXT_DIM))
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
                                            .h(px(18.0))
                                            .px(px(12.0))
                                            .flex()
                                            .items_center()
                                            .font_family("Menlo")
                                            .children(line.spans.iter().map(|span| {
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(span.color))
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
                    .bg(rgb(theme::STATUS_BAR_BG))
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_DIM))
                            .child(status_text),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_DIM))
                            .child("\u{2191}\u{2193} navigate  \u{23CE} open  esc quit"),
                    ),
            )
    }
}
