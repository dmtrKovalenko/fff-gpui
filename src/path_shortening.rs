use std::borrow::Cow;
use std::path::{Component, Path};

#[derive(Clone, Copy)]
pub enum PathShortenStrategy {
    MiddleNumber,
}

impl PathShortenStrategy {
    // Shorten a path to fit within the requested character budget.
    pub fn shorten_path(&self, path: &Path, max_size: usize) -> String {
        const MIN_SMART_SHORTEN_SIZE: usize = 8;

        let sep = std::path::MAIN_SEPARATOR;
        let path_str = path.to_string_lossy();
        if path_str.len() <= max_size {
            return path_str.to_string();
        }

        if max_size < MIN_SMART_SHORTEN_SIZE {
            return Self::truncate_str(&path_str, max_size);
        }

        let components: Vec<&str> = path
            .components()
            .filter_map(|component| match component {
                Component::Normal(segment) => segment.to_str(),
                _ => None,
            })
            .collect();

        match components.len() {
            0 => path_str.to_string(),
            1 => Self::truncate_str(components[0], max_size),
            2 => Self::shorten_pair(&components, max_size, sep),
            _ => self.shorten_middle(&components, max_size, sep),
        }
    }

    // Shorten a two-component path while preserving the filename when possible.
    fn shorten_pair(components: &[&str], max_size: usize, sep: char) -> String {
        let joined = components.join(&sep.to_string());
        if joined.len() <= max_size {
            return joined;
        }

        let last = components[1];
        let available_for_first = max_size.saturating_sub(1 + last.len());
        if available_for_first > 0 && last.len() < max_size {
            return format!(
                "{}{}{}",
                Self::truncate_str(components[0], available_for_first),
                sep,
                last
            );
        }

        Self::truncate_str(last, max_size)
    }

    // Shorten a multi-component path by replacing middle components with a marker.
    fn shorten_middle(&self, components: &[&str], max_size: usize, sep: char) -> String {
        let total = components.len();
        let first = components[0];
        let last = components[total - 1];
        let hidden = total - 2;
        let ellipsis = Self::make_ellipsis(hidden);
        let min_overhead = 2 + ellipsis.len();

        if first.len() + last.len() + min_overhead <= max_size {
            return self.expand_middle(components, max_size, sep);
        }

        let needed_for_last = last.len() + 1 + ellipsis.len() + 1;
        if needed_for_last <= max_size {
            let available_for_first = max_size - needed_for_last;
            return format!(
                "{}{}{}{}{}",
                Self::truncate_str(first, available_for_first),
                sep,
                ellipsis,
                sep,
                last
            );
        }

        let needed_for_ellipsis_last = ellipsis.len() + 1 + last.len();
        if needed_for_ellipsis_last <= max_size {
            return format!("{}{}{}", ellipsis, sep, last);
        }

        Self::truncate_str(last, max_size)
    }

    // Expand the visible prefix and suffix while the path still fits.
    fn expand_middle(&self, components: &[&str], max_size: usize, sep: char) -> String {
        let total = components.len();
        let mut left_end = 1;
        let mut right_start = total - 1;

        loop {
            if right_start <= left_end {
                break;
            }

            let mut added = false;
            if right_start > left_end + 1 {
                let hidden = right_start - 1 - left_end;
                let candidate =
                    Self::build_middle_result(components, left_end, right_start - 1, hidden, sep);
                if candidate.len() <= max_size {
                    right_start -= 1;
                    added = true;
                }
            }

            if left_end < right_start - 1 {
                let hidden = right_start - (left_end + 1);
                let candidate =
                    Self::build_middle_result(components, left_end + 1, right_start, hidden, sep);
                if candidate.len() <= max_size {
                    left_end += 1;
                    added = true;
                }
            }

            if !added {
                break;
            }
        }

        Self::build_middle_result(
            components,
            left_end,
            right_start,
            right_start - left_end,
            sep,
        )
    }

    // Build a shortened path from visible prefix, marker, and suffix components.
    fn build_middle_result(
        components: &[&str],
        left_end: usize,
        right_start: usize,
        hidden_count: usize,
        sep: char,
    ) -> String {
        let ellipsis = Self::make_ellipsis(hidden_count);
        let mut result = String::new();

        for (idx, part) in components[..left_end].iter().enumerate() {
            if idx > 0 {
                result.push(sep);
            }
            result.push_str(part);
        }

        if left_end > 0 {
            result.push(sep);
        }
        result.push_str(&ellipsis);
        result.push(sep);

        for (idx, part) in components[right_start..].iter().enumerate() {
            if idx > 0 {
                result.push(sep);
            }
            result.push_str(part);
        }

        result
    }

    // Format the marker that represents hidden path components.
    fn make_ellipsis(hidden_count: usize) -> Cow<'static, str> {
        match hidden_count {
            1 => ".".into(),
            2 => "..".into(),
            3 => "...".into(),
            n => format!(".{}.", n).into(),
        }
    }

    // Truncate a string to a maximum number of characters.
    fn truncate_str(s: &str, max_len: usize) -> String {
        if max_len == 0 {
            return String::new();
        }

        s.chars().take(max_len).collect()
    }
}
