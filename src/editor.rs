use std::path::Path;
use std::process::{Child, Command};

use anyhow::{Context as _, anyhow};
use tracing::{debug, info, instrument};

// Spawn the user's $EDITOR or $VISUAL with the selected file.
#[instrument(skip(path), fields(path = %path.display(), line = ?line))]
pub fn open_in_editor(path: &Path, line: Option<usize>) -> anyhow::Result<Child> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_| {
            anyhow!(
                "Neither $EDITOR nor $VISUAL is set; refusing to guess an editor for {}",
                path.display()
            )
        })?;

    info!(editor = ?editor, "opening file with editor");

    editor_command(&editor, path, line)
        .spawn()
        .with_context(|| format!("failed to spawn editor command {:?}", editor))
}

// Build a shell command when the editor contains arguments, otherwise run it directly.
fn editor_command(editor: &str, path: &Path, line: Option<usize>) -> Command {
    if editor.split_whitespace().nth(1).is_some() {
        debug!(editor = ?editor, "editor command uses shell wrapper");
        let mut command = Command::new("sh");
        let goto_line = line.map(|line| format!(" +{line}")).unwrap_or_default();
        command
            .arg("-c")
            .arg(format!("exec {}{} \"$1\"", editor, goto_line))
            .arg("fff-editor")
            .arg(path);
        command
    } else {
        let mut command = Command::new(editor);
        if let Some(line) = line {
            command.arg(format!("+{line}"));
        }
        command.arg(path);
        command
    }
}
