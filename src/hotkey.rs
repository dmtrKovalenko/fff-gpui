use anyhow::{Result, anyhow};
use async_channel::Sender;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

use crate::service::{CommandEnvelope, ServiceCommand};

pub fn install_event_handler(command_tx: Sender<CommandEnvelope>) {
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state() == HotKeyState::Pressed {
            let _ = command_tx.send_blocking((ServiceCommand::ToggleWindow, None));
        }
    }));
}

pub fn parse_hotkey(binding: Option<&str>) -> Result<Option<HotKey>> {
    let Some(binding) = binding else {
        return Ok(None);
    };

    let trimmed = binding.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if trimmed.split_whitespace().count() != 1 {
        return Err(anyhow!(
            "global hotkey must be a single shortcut, got {:?}",
            binding
        ));
    }

    let normalized = trimmed.replace('-', "+");
    let hotkey = normalized
        .parse::<HotKey>()
        .map_err(|err| anyhow!("invalid global hotkey {:?}: {}", binding, err))?;
    Ok(Some(hotkey))
}
