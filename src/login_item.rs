use anyhow::{Context as _, Result, anyhow};
use tracing::{info, warn};

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CStr;
    use std::os::raw::c_char;

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}

    fn error_message(error: id) -> Option<String> {
        if error == nil {
            return None;
        }

        unsafe {
            let description: id = msg_send![error, localizedDescription];
            if description == nil {
                return None;
            }

            let cstr: *const c_char = msg_send![description, UTF8String];
            (!cstr.is_null()).then(|| CStr::from_ptr(cstr).to_string_lossy().into_owned())
        }
    }

    pub fn set_launch_at_login(enabled: bool) -> Result<()> {
        unsafe {
            let service: id = msg_send![class!(SMAppService), mainApp];
            if service == nil {
                return Err(anyhow!("SMAppService.mainApp is unavailable"));
            }

            let mut error: id = nil;
            let success: bool = if enabled {
                msg_send![service, registerAndReturnError: &mut error]
            } else {
                msg_send![service, unregisterAndReturnError: &mut error]
            };

            if success {
                info!(enabled, "updated login item registration");
                return Ok(());
            }

            let message = error_message(error).unwrap_or_else(|| "unknown error".to_string());
            Err(anyhow!(message))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use super::*;

    pub fn set_launch_at_login(enabled: bool) -> Result<()> {
        let _ = enabled;
        Ok(())
    }
}

pub fn apply_launch_at_login(enabled: bool) -> Result<()> {
    macos::set_launch_at_login(enabled).context("failed to update launch-at-login setting")
}

pub fn sync_launch_at_login(enabled: bool) {
    if let Err(err) = apply_launch_at_login(enabled) {
        warn!(enabled, error = %err, "unable to apply launch-at-login preference");
    }
}
