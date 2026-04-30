use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

// Return the log path used for open failures and spawn traces.
fn log_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join(".local/state/fff-gpui/fff-gpui.log")
}

// Append a message to stderr and the background log writer.
pub fn append(message: impl AsRef<str>) {
    let message = message.as_ref().to_string();
    if sender().send(message.clone()).is_err() {
        eprintln!("{message}");
    }
}

// Return the singleton channel feeding the log writer thread.
fn sender() -> &'static Sender<String> {
    static SENDER: OnceLock<Sender<String>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("fff-gpui-log".to_string())
            .spawn(move || {
                while let Ok(message) = rx.recv() {
                    write_message(&message);
                }
            })
            .expect("failed to spawn fff-gpui log writer");
        tx
    })
}

// Write a single log message to stderr and disk.
fn write_message(message: &str) {
    eprintln!("{message}");

    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

// Return the log path as display text for user-facing errors.
pub fn path_for_display() -> String {
    log_path().display().to_string()
}
