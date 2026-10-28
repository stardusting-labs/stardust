//! Start-at-login entries.
//!
//! Off until the user turns it on. Each platform gets one per-user entry that
//! starts `stardust` with no arguments:
//!
//! - Linux and other XDG desktops: `$XDG_CONFIG_HOME/autostart/stardust.desktop`.
//! - macOS: `~/Library/LaunchAgents/app.helloorbit.stardust.plist`.
//! - Windows: the `Stardust` value under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! This module only builds text and paths. The app writes the entry.

use std::path::{Path, PathBuf};

pub const LAUNCH_AGENT_LABEL: &str = "app.helloorbit.stardust";
pub const WINDOWS_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const WINDOWS_RUN_VALUE: &str = "Stardust";
pub const DESKTOP_FILE_NAME: &str = "stardust.desktop";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Xdg,
    MacOs,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Xdg
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutostartError {
    #[error("the app path cannot be used for start at login")]
    ExecutablePath,
    #[error("the home folder is not known")]
    Home,
    #[error("the start-at-login entry could not be written")]
    Write,
    #[error("move Stardust to the Applications folder, then turn on start at login")]
    Translocated,
}

/// Where the per-user entry lives. Windows uses the registry, so it has no file.
pub fn entry_file(
    platform: Platform,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
) -> Result<Option<PathBuf>, AutostartError> {
    match platform {
        Platform::Windows => Ok(None),
        Platform::MacOs => {
            let home = home
                .filter(|value| !value.is_empty())
                .ok_or(AutostartError::Home)?;
            Ok(Some(
                PathBuf::from(home)
                    .join("Library")
                    .join("LaunchAgents")
                    .join(format!("{LAUNCH_AGENT_LABEL}.plist")),
            ))
        }
        Platform::Xdg => {
            if let Some(xdg) = xdg_config_home.filter(|value| !value.is_empty()) {
                return Ok(Some(
                    PathBuf::from(xdg).join("autostart").join(DESKTOP_FILE_NAME),
                ));
            }
            let home = home
                .filter(|value| !value.is_empty())
                .ok_or(AutostartError::Home)?;
            Ok(Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("autostart")
                    .join(DESKTOP_FILE_NAME),
            ))
        }
    }
}

fn checked_exe(exe: &Path) -> Result<&str, AutostartError> {
    let text = exe.to_str().ok_or(AutostartError::ExecutablePath)?;
    if text.is_empty() || !exe.is_absolute() || text.chars().any(char::is_control) {
        return Err(AutostartError::ExecutablePath);
    }
    Ok(text)
}

/// XDG desktop entry.
///
/// The path is quoted for `Exec`. A path with a character that needs a
/// second level of Desktop Entry escaping (`"`, `` ` ``, `$`, `\`) is refused
/// instead of guessed.
pub fn desktop_entry(exe: &Path) -> Result<String, AutostartError> {
    let exe = checked_exe(exe)?;
    if exe.contains(['"', '`', '$', '\\']) {
        return Err(AutostartError::ExecutablePath);
    }
    let quoted = format!("\"{}\"", exe.replace('%', "%%"));
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=Stardust\nComment=ORBIT tray companion\nExec={quoted}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    ))
}

/// macOS per-user LaunchAgent. `RunAtLoad` starts it at login.
///
/// An app opened straight from a downloaded zip runs from a random
/// App Translocation path that is gone after a restart, so that path is
/// refused.
pub fn launch_agent(exe: &Path) -> Result<String, AutostartError> {
    let exe = checked_exe(exe)?;
    if exe.contains("/AppTranslocation/") {
        return Err(AutostartError::Translocated);
    }
    let exe = xml_escape(exe);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{LAUNCH_AGENT_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{exe}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Interactive</string>\n</dict>\n</plist>\n"
    ))
}

/// Windows Run value data: the quoted exe path.
pub fn windows_run_data(exe: &Path) -> Result<String, AutostartError> {
    let exe = checked_exe(exe)?;
    if exe.contains('"') {
        return Err(AutostartError::ExecutablePath);
    }
    Ok(format!("\"{exe}\""))
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Entry text for a file-based platform.
pub fn entry_text(platform: Platform, exe: &Path) -> Result<Option<String>, AutostartError> {
    match platform {
        Platform::Xdg => desktop_entry(exe).map(Some),
        Platform::MacOs => launch_agent(exe).map(Some),
        Platform::Windows => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        desktop_entry, entry_file, launch_agent, windows_run_data, AutostartError, Platform,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn entry_files_follow_each_platform_convention() {
        assert_eq!(
            entry_file(Platform::Xdg, Some("/x"), Some("/h")).unwrap(),
            Some(PathBuf::from("/x/autostart/stardust.desktop"))
        );
        assert_eq!(
            entry_file(Platform::Xdg, Some(""), Some("/h")).unwrap(),
            Some(PathBuf::from("/h/.config/autostart/stardust.desktop"))
        );
        assert_eq!(
            entry_file(Platform::MacOs, None, Some("/Users/a")).unwrap(),
            Some(PathBuf::from(
                "/Users/a/Library/LaunchAgents/app.helloorbit.stardust.plist"
            ))
        );
        assert_eq!(entry_file(Platform::Windows, None, None).unwrap(), None);
        assert_eq!(
            entry_file(Platform::Xdg, None, None).unwrap_err(),
            AutostartError::Home
        );
    }

    #[test]
    fn desktop_entry_quotes_the_exec_path() {
        let text = desktop_entry(Path::new("/opt/My Apps/stardust 100%/stardust")).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("Exec=\"/opt/My Apps/stardust 100%%/stardust\"\n"));
        assert!(desktop_entry(Path::new("/opt/star$dust/stardust")).is_err());
        assert!(text.contains("Terminal=false\n"));
        assert!(desktop_entry(Path::new("relative/stardust")).is_err());
        assert!(desktop_entry(Path::new("/opt/bad\nExec=evil")).is_err());
    }

    #[test]
    fn launch_agent_escapes_xml_and_runs_at_load() {
        let text = launch_agent(Path::new(
            "/Applications/A&B <x>.app/Contents/MacOS/stardust",
        ))
        .unwrap();
        assert!(text.contains(
            "<string>/Applications/A&amp;B &lt;x&gt;.app/Contents/MacOS/stardust</string>"
        ));
        assert!(text.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(text.contains("<string>app.helloorbit.stardust</string>"));
        assert_eq!(
            launch_agent(Path::new(
                "/private/var/folders/x/AppTranslocation/1234/d/Stardust.app/Contents/MacOS/stardust"
            ))
            .unwrap_err(),
            AutostartError::Translocated
        );
    }

    #[test]
    fn windows_run_data_is_one_quoted_path() {
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\Program Files\Stardust\stardust.exe")
        } else {
            PathBuf::from("/c/Program Files/Stardust/stardust.exe")
        };
        let data = windows_run_data(&exe).unwrap();
        assert!(data.starts_with('"') && data.ends_with('"'));
        assert_eq!(data.matches('"').count(), 2);
    }
}
