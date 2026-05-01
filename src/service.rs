use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::thread;

use anyhow::{Context as _, Result};
use async_channel::Sender;
use tracing::{debug, info, instrument, warn};

#[derive(Debug, Clone)]
pub enum ServiceCommand {
    OpenPath { path: PathBuf, in_grep: bool },
    OpenOneShot { path: PathBuf, in_grep: bool },
    OpenConfig,
    ShowPicker,
    ToggleWindow,
    Quit,
}

fn socket_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".local/state/fff-gpui/fff-gpui.sock")
}

#[cfg(unix)]
fn read_request(line: String) -> Option<ServiceCommand> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    fn split_path_and_grep(payload: &str) -> (PathBuf, bool) {
        match payload.split_once('\t') {
            Some((path, "grep")) => (PathBuf::from(path), true),
            _ => (PathBuf::from(payload), false),
        }
    }

    if let Some(payload) = trimmed.strip_prefix("open\t") {
        let (path, in_grep) = split_path_and_grep(payload);
        return Some(ServiceCommand::OpenOneShot { path, in_grep });
    }

    if trimmed == "config" {
        return Some(ServiceCommand::OpenConfig);
    }

    if let Some(payload) = trimmed.strip_prefix("path\t") {
        let (path, in_grep) = split_path_and_grep(payload);
        return Some(ServiceCommand::OpenPath { path, in_grep });
    }

    match trimmed {
        "show" => return Some(ServiceCommand::ShowPicker),
        "toggle" => return Some(ServiceCommand::ToggleWindow),
        "quit" => return Some(ServiceCommand::Quit),
        _ => {}
    }

    (!trimmed.is_empty()).then(|| ServiceCommand::OpenPath {
        path: PathBuf::from(trimmed),
        in_grep: false,
    })
}

#[cfg(not(unix))]
fn read_request(_line: String) -> Option<ServiceCommand> {
    None
}

fn write_request(stream: &mut impl Write, command: &ServiceCommand) -> Result<()> {
    fn grep_suffix(in_grep: bool) -> &'static str {
        if in_grep { "\tgrep" } else { "" }
    }

    match command {
        ServiceCommand::OpenPath { path, in_grep } => writeln!(
            stream,
            "path\t{}{}",
            path.display(),
            grep_suffix(*in_grep)
        ),
        ServiceCommand::OpenOneShot { path, in_grep } => writeln!(
            stream,
            "open\t{}{}",
            path.display(),
            grep_suffix(*in_grep)
        ),
        ServiceCommand::OpenConfig => writeln!(stream, "config"),
        ServiceCommand::ShowPicker => writeln!(stream, "show"),
        ServiceCommand::ToggleWindow => writeln!(stream, "toggle"),
        ServiceCommand::Quit => writeln!(stream, "quit"),
    }
    .context("failed to send launch request")
}

/// Try to forward this launch to a running service instance.
#[instrument(skip(command), fields(command = ?command))]
pub fn forward_to_running_instance(command: &ServiceCommand) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;

        let path = socket_path();
        debug!(socket = %path.display(), "trying to connect to resident service");
        match UnixStream::connect(&path) {
            Ok(mut stream) => {
                write_request(&mut stream, command)?;
                let _ = stream.flush();
                info!("forwarded launch request to resident service");
                Ok(true)
            }
            Err(err) => {
                debug!(error = %err, "no resident service available");
                Ok(false)
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = command;
        Ok(false)
    }
}

/// Start the background listener that receives launch requests from subsequent invocations.
#[instrument(skip(commands))]
pub fn start_listener(commands: Sender<ServiceCommand>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::net::UnixListener;

        let path = socket_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("failed to create service directory")?;
        }
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind service socket at {}", path.display()))?;
        info!(socket = %path.display(), "service listener bound");

        thread::Builder::new()
            .name("fff-gpui-ipc".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else {
                        warn!("incoming service request stream failed");
                        continue;
                    };
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok()
                        && let Some(request) = read_request(line)
                    {
                        debug!(?request, "received service request");
                        let _ = commands.send_blocking(request);
                    }
                }
            })
            .context("failed to spawn launch request listener")?;
    }

    #[cfg(not(unix))]
    {
        let _ = commands;
    }

    Ok(())
}
