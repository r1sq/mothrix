// SPDX-License-Identifier: MPL-2.0

//! Desktop notifications on Windows: toasts. Windows shows a desktop program's toasts under the name
//! and icon registered for its app id
//! (`HKCU\Software\Classes\AppUserModelId\…`), which mothrix writes itself
//! before the first; no installer or admin rights needed. A click on the
//! toast counts while it is on screen, not later from the notification
//! centre.

use std::path::PathBuf;
use std::sync::OnceLock;

use tauri_winrt_notification::{Duration, Sound, Toast};

/// The app id toasts are shown under; mothrix registers it with Windows
/// itself on the first toast.
const APP_ID: &str = "app.mothrix";

/// The moth, as the toasts' icon.
const ICON: &[u8] = include_bytes!("../../../assets/mothrix-128.png");

/// Register the app id with mothrix's name and icon, for this user. Best
/// effort: without it, toasts go out under PowerShell's id.
fn register() -> bool {
    let icon = icon_file();
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    match key {
        Ok((key, _)) => {
            let named = key.set_value("DisplayName", &"mothrix").is_ok();
            if let Some(icon) = icon {
                let _ = key.set_value("IconUri", &icon.display().to_string());
            }
            named
        }
        Err(_) => false,
    }
}

/// The moth as a picture file Windows can show in the toast.
fn icon_file() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("mothrix");
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join("mothrix.png");
    std::fs::write(&file, ICON).ok()?;
    Some(file)
}

/// Show a toast about new messages in a room called `title`; `clicked`
/// runs when the user clicks it. Blocks a moment: call it off the UI's
/// thread.
pub fn show(title: &str, clicked: impl Fn() + Send + 'static) {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let app_id = if *REGISTERED.get_or_init(register) {
        APP_ID
    } else {
        Toast::POWERSHELL_APP_ID
    };
    let result = Toast::new(app_id)
        .title(title)
        .text1("New message")
        .sound(Some(Sound::Default))
        .duration(Duration::Short)
        .on_activated(move |_| {
            clicked();
            Ok(())
        })
        .show();
    if let Err(error) = result {
        tracing::warn!(%error, "could not show a notification");
    }
}
