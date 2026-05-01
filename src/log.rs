use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
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

// Initialize tracing for stderr and the persistent log file.
pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_filter(env_filter);

    let file_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(LogFileWriter)
        .with_filter(LevelFilter::TRACE);

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();
}

// Return the log path as display text for user-facing errors.
pub fn path_for_display() -> String {
    log_path().display().to_string()
}
