use std::path::Path;
use std::process::{Child, Command};

use anyhow::{Context as _, anyhow};

use crate::log;

// Spawn the user's $EDITOR or $VISUAL with the selected file.
pub fn open_in_editor(path: &Path) -> anyhow::Result<Child> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_| {
            anyhow!(
                "Neither $EDITOR nor $VISUAL is set; refusing to guess an editor for {}",
                path.display()
            )
        })?;

    log::append(format!(
        "fff-gpui: opening {} with editor command {:?}",
        path.display(),
        editor
    ));

    editor_command(&editor, path)
        .spawn()
        .with_context(|| format!("failed to spawn editor command {:?}", editor))
}

// Build a shell command when the editor contains arguments, otherwise run it directly.
fn editor_command(editor: &str, path: &Path) -> Command {
    if editor.split_whitespace().nth(1).is_some() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("exec {} \"$1\"", editor))
            .arg("fff-editor")
            .arg(path);
        command
    } else {
        let mut command = Command::new(editor);
        command.arg(path);
        command
    }
}
