use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;

// Return the log path used for file-backed tracing output.
fn log_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join(".local/state/fff-gpui/fff-gpui.log")
}

struct LogFileWriter;

impl<'a> MakeWriter<'a> for LogFileWriter {
    type Writer = Box<dyn Write + Send + 'static>;

    fn make_writer(&'a self) -> Self::Writer {
        let path = log_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => Box::new(file),
            Err(_) => Box::new(io::sink()),
        }
    }
}

// Build a filter from an app-specific env var, falling back to a sane default
// that keeps our own logs useful without pulling in dependency noise.
fn env_filter(var_name: &str, default_directives: &str) -> EnvFilter {
    std::env::var(var_name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            default_directives
                .parse()
                .expect("default tracing directives must be valid")
        })
}

// Initialize tracing for stderr and the persistent log file.
pub fn init_tracing() {
    let stderr_filter = env_filter(
        "FFF_GPUI_LOG",
        "fff_gpui=info,fff_search=info,fff_query_parser=warn,fff_grep=warn,gpui=warn,ignore=warn,smol=warn",
    );
    let file_filter = env_filter(
        "FFF_GPUI_FILE_LOG",
        "fff_gpui=debug,fff_search=info,fff_query_parser=warn,fff_grep=warn,gpui=info,ignore=warn,smol=warn",
    );

    let stderr_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_filter(stderr_filter);

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(LogFileWriter)
        .with_filter(file_filter);

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();
}

// Return the log path as display text for user-facing errors.
pub fn path_for_display() -> String {
    log_path().display().to_string()
}
