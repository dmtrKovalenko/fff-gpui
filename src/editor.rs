use std::path::Path;
use std::process::{Child, Command};

use anyhow::{Context as _, anyhow};
use tracing::{debug, info, instrument};

// Spawn the user's $EDITOR or $VISUAL with the selected file.
#[instrument(skip(path), fields(path = %path.display(), goto = ?goto))]
pub fn open_in_editor(path: &Path, goto: Option<(usize, usize)>) -> anyhow::Result<Child> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_| {
            anyhow!(
                "Neither $EDITOR nor $VISUAL is set; refusing to guess an editor for {}",
                path.display()
            )
        })?;

    info!(editor = ?editor, "opening file with editor");

    editor_command(&editor, path, goto)
        .spawn()
        .with_context(|| format!("failed to spawn editor command {:?}", editor))
}

#[derive(Clone, Copy)]
enum LineFormat {
    // vim/nvim/emacs/nano: `editor +line path`
    Plus,
    // zed/subl/mate: `editor path:line:column` (positional)
    Colon,
    // code/cursor/codium: `editor -g path:line:column` (needs --goto flag)
    GotoFlag,
}

fn line_format(editor: &str) -> LineFormat {
    let bin = editor.split_whitespace().next().unwrap_or("");
    let basename = Path::new(bin)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(bin)
        .to_lowercase();
    match basename.as_str() {
        "zed" | "zed-preview" | "subl" | "sublime_text" | "mate" => LineFormat::Colon,
        "code" | "code-insiders" | "cursor" | "codium" | "vscodium" => LineFormat::GotoFlag,
        _ => LineFormat::Plus,
    }
}

// Build a shell command when the editor contains arguments, otherwise run it directly.
fn editor_command(editor: &str, path: &Path, goto: Option<(usize, usize)>) -> Command {
    let format = line_format(editor);
    let has_args = editor.split_whitespace().nth(1).is_some();

    if has_args {
        debug!(editor = ?editor, "editor command uses shell wrapper");
        let mut command = Command::new("sh");
        match (format, goto) {
            (LineFormat::Plus, Some((line, _))) => {
                command
                    .arg("-c")
                    .arg(format!("exec {editor} +{line} \"$1\""))
                    .arg("fff-editor")
                    .arg(path);
            }
            (LineFormat::Colon, Some((line, column))) => {
                command
                    .arg("-c")
                    .arg(format!("exec {editor} \"$1\""))
                    .arg("fff-editor")
                    .arg(format!("{}:{line}:{column}", path.display()));
            }
            (LineFormat::GotoFlag, Some((line, column))) => {
                command
                    .arg("-c")
                    .arg(format!("exec {editor} -g \"$1\""))
                    .arg("fff-editor")
                    .arg(format!("{}:{line}:{column}", path.display()));
            }
            (_, None) => {
                command
                    .arg("-c")
                    .arg(format!("exec {editor} \"$1\""))
                    .arg("fff-editor")
                    .arg(path);
            }
        }
        command
    } else {
        let mut command = Command::new(editor);
        match (format, goto) {
            (LineFormat::Plus, Some((line, _))) => {
                command.arg(format!("+{line}")).arg(path);
            }
            (LineFormat::Colon, Some((line, column))) => {
                command.arg(format!("{}:{line}:{column}", path.display()));
            }
            (LineFormat::GotoFlag, Some((line, column))) => {
                command
                    .arg("-g")
                    .arg(format!("{}:{line}:{column}", path.display()));
            }
            (_, None) => {
                command.arg(path);
            }
        }
        command
    }
}
