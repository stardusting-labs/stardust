//! Write or remove the start-at-login entry for this user.
//!
//! Linux and macOS use a file. Windows uses `reg.exe` on the per-user Run
//! key, so no extra crate and no unsafe code are needed. Nothing here runs
//! unless the user turns the option on.

use stardust_core::autostart::{entry_file, entry_text, AutostartError, Platform};
use std::fs;
use std::path::{Path, PathBuf};

pub fn available() -> bool {
    cfg!(any(unix, windows))
}

fn current_exe() -> Result<PathBuf, AutostartError> {
    std::env::current_exe().map_err(|_| AutostartError::ExecutablePath)
}

fn current_entry_file() -> Result<Option<PathBuf>, AutostartError> {
    entry_file(
        Platform::current(),
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

pub fn is_enabled() -> bool {
    match Platform::current() {
        Platform::Windows => windows::is_enabled(),
        Platform::Xdg | Platform::MacOs => current_entry_file()
            .ok()
            .flatten()
            .is_some_and(|path| path.is_file()),
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), AutostartError> {
    let platform = Platform::current();
    if platform == Platform::Windows {
        return if enabled {
            windows::enable(&current_exe()?)
        } else {
            windows::disable()
        };
    }
    let path = current_entry_file()?.ok_or(AutostartError::Home)?;
    if enabled {
        write_entry(platform, &path, &current_exe()?)
    } else {
        remove_entry(&path)
    }
}

/// Write the entry file for `exe` at `path`.
pub fn write_entry(platform: Platform, path: &Path, exe: &Path) -> Result<(), AutostartError> {
    let Some(text) = entry_text(platform, exe)? else {
        return Err(AutostartError::Write);
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| AutostartError::Write)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, text).map_err(|_| AutostartError::Write)?;
    fs::rename(&tmp, path).map_err(|_| {
        let _ = fs::remove_file(&tmp);
        AutostartError::Write
    })
}

pub fn remove_entry(path: &Path) -> Result<(), AutostartError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AutostartError::Write),
    }
}

mod windows {
    use stardust_core::autostart::{
        windows_run_data, AutostartError, WINDOWS_RUN_KEY, WINDOWS_RUN_VALUE,
    };
    use std::path::Path;
    use std::process::{Command, Stdio};

    fn reg(args: &[&str]) -> bool {
        Command::new("reg")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    pub fn is_enabled() -> bool {
        cfg!(windows) && reg(&["query", WINDOWS_RUN_KEY, "/v", WINDOWS_RUN_VALUE])
    }

    pub fn enable(exe: &Path) -> Result<(), AutostartError> {
        let data = windows_run_data(exe)?;
        if cfg!(windows)
            && reg(&[
                "add",
                WINDOWS_RUN_KEY,
                "/v",
                WINDOWS_RUN_VALUE,
                "/t",
                "REG_SZ",
                "/d",
                &data,
                "/f",
            ])
        {
            Ok(())
        } else {
            Err(AutostartError::Write)
        }
    }

    pub fn disable() -> Result<(), AutostartError> {
        if !cfg!(windows) || !is_enabled() {
            return Ok(());
        }
        if reg(&["delete", WINDOWS_RUN_KEY, "/v", WINDOWS_RUN_VALUE, "/f"]) {
            Ok(())
        } else {
            Err(AutostartError::Write)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_entry, write_entry};
    use stardust_core::autostart::{entry_file, Platform};
    use std::path::PathBuf;

    #[test]
    fn writes_and_removes_the_xdg_and_macos_entries() {
        let root = std::env::temp_dir().join(format!("stardust-autostart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let exe = PathBuf::from("/opt/stardust/stardust");
        let xdg = entry_file(Platform::Xdg, root.join("xdg").to_str(), None)
            .unwrap()
            .unwrap();
        write_entry(Platform::Xdg, &xdg, &exe).unwrap();
        let text = std::fs::read_to_string(&xdg).unwrap();
        assert!(text.contains("Exec=\"/opt/stardust/stardust\""));
        write_entry(Platform::Xdg, &xdg, &exe).unwrap();
        remove_entry(&xdg).unwrap();
        assert!(!xdg.exists());
        remove_entry(&xdg).unwrap();

        let mac = entry_file(Platform::MacOs, None, root.join("home").to_str())
            .unwrap()
            .unwrap();
        write_entry(Platform::MacOs, &mac, &exe).unwrap();
        assert!(std::fs::read_to_string(&mac)
            .unwrap()
            .contains("<string>/opt/stardust/stardust</string>"));
        remove_entry(&mac).unwrap();
        assert!(write_entry(Platform::Windows, &root.join("x"), &exe).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
