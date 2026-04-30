use std::path::Path;
use std::sync::OnceLock;

use crate::theme;

use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

pub const MAX_PREVIEW_LINES: usize = 500;
pub const MATCH_CONTEXT_BEFORE: usize = 8;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<two_face::theme::EmbeddedLazyThemeSet> = OnceLock::new();

// Return the expanded syntax set used by preview highlighting.
fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

// Return the preview theme used for highlighted spans.
fn preview_theme() -> &'static Theme {
    THEME_SET
        .get_or_init(two_face::theme::extra)
        .get(EmbeddedThemeName::Base16OceanDark)
}

// Resolve a syntax from a file path, falling back to plain text.
fn syntax_for_path<'a>(ps: &'a SyntaxSet, path: &Path) -> &'a SyntaxReference {
    ps.find_syntax_for_file(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| ps.find_syntax_plain_text())
}

// Warm the syntax and theme caches before the first preview render.
pub fn warm_highlighter() {
    let _ = syntax_set();
    let _ = preview_theme();
}

#[derive(Clone)]
pub struct HighlightedSpan {
    pub color: u32,
    pub text: String,
}

#[derive(Clone)]
pub struct HighlightedLine {
    pub spans: Vec<HighlightedSpan>,
}

fn clamp_range_to_char_boundaries(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut start = start.min(text.len());
    let mut end = end.min(text.len());

    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    (start < end).then_some((start, end))
}

// Overlay grep match ranges onto already-highlighted spans.
pub fn overlay_match_ranges(
    spans: &[HighlightedSpan],
    byte_ranges: &[(u32, u32)],
) -> Vec<HighlightedSpan> {
    if byte_ranges.is_empty() {
        return spans.to_vec();
    }

    let mut sorted_ranges = byte_ranges.to_vec();
    sorted_ranges.sort_unstable_by_key(|&(s, _)| s);

    let mut result = Vec::new();
    let mut byte_pos: u32 = 0;

    for span in spans {
        let span_start = byte_pos;
        let span_end = span_start + span.text.len() as u32;
        let mut chunk_start = span_start;

        for &(range_start, range_end) in &sorted_ranges {
            let overlap_start = range_start.max(chunk_start);
            let overlap_end = range_end.min(span_end);

            if overlap_start >= overlap_end {
                continue;
            }

            let pre_s = (chunk_start - span_start) as usize;
            let pre_e = (overlap_start - span_start) as usize;
            if let Some((pre_s, pre_e)) = clamp_range_to_char_boundaries(&span.text, pre_s, pre_e) {
                result.push(HighlightedSpan {
                    color: span.color,
                    text: span.text[pre_s..pre_e].to_string(),
                });
            }

            let hi_s = (overlap_start - span_start) as usize;
            let hi_e = (overlap_end - span_start) as usize;
            if let Some((hi_s, hi_e)) = clamp_range_to_char_boundaries(&span.text, hi_s, hi_e) {
                result.push(HighlightedSpan {
                    color: theme::MATCH_HIGHLIGHT,
                    text: span.text[hi_s..hi_e].to_string(),
                });
            }

            chunk_start = overlap_end;
        }

        let tail_s = (chunk_start - span_start) as usize;
        if let Some((tail_s, tail_e)) =
            clamp_range_to_char_boundaries(&span.text, tail_s, span.text.len())
        {
            result.push(HighlightedSpan {
                color: span.color,
                text: span.text[tail_s..tail_e].to_string(),
            });
        }

        byte_pos = span_end;
    }

    result
}

// Read a file and return a syntax-highlighted preview window.
pub fn highlight_file_window(
    path: &Path,
    center_line: Option<usize>,
) -> (usize, Vec<HighlightedLine>) {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return (1, vec![]),
    };

    let ps = syntax_set();
    let theme = preview_theme();

    let syntax = syntax_for_path(ps, path);
    let mut h = HighlightLines::new(syntax, theme);
    let mut lines = Vec::with_capacity(MAX_PREVIEW_LINES);
    let start_line = center_line
        .map(|line| line.saturating_sub(MATCH_CONTEXT_BEFORE).max(1))
        .unwrap_or(1);

    for (idx, line_str) in LinesWithEndings::from(&content).enumerate() {
        let ranges = match h.highlight_line(line_str, ps) {
            Ok(r) => r,
            Err(_) => break,
        };

        let line_number = idx + 1;
        if line_number < start_line {
            continue;
        }
        if lines.len() >= MAX_PREVIEW_LINES {
            break;
        }

        let spans = ranges
            .into_iter()
            .filter_map(|(style, text)| {
                let t = text.trim_end_matches(['\n', '\r']);
                if t.is_empty() {
                    return None;
                }
                let c = style.foreground;
                Some(HighlightedSpan {
                    color: ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32),
                    text: t.to_string(),
                })
            })
            .collect();

        lines.push(HighlightedLine { spans });
    }

    (start_line, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify that the expanded syntax bundle handles TypeScript paths.
    #[test]
    fn extended_syntax_set_detects_typescript_files() {
        let ps = syntax_set();

        for file_name in ["component.ts", "component.tsx"] {
            let syntax = syntax_for_path(ps, Path::new(file_name));

            assert_ne!(
                syntax.name, "Plain Text",
                "{file_name} should use a real syntax"
            );
        }
    }
}
