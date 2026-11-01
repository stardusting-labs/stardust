//! Reading the OS do-not-disturb switch, best effort.
//!
//! Used only with routing on, before the bubble or the OS notification. On
//! means local alerts wait until it is off; it never makes anything louder. These
//! parse what the app reads: GNOME's `show-banners` setting, the Windows
//! global notifications switch, and the macOS Focus assertions file. Windows
//! Focus Assist priority modes and KDE are not detected. Anything that cannot
//! be read is `None`, which the app treats as off.

use serde_json::Value;

/// Output of `gsettings get org.gnome.desktop.notifications show-banners`.
/// Banners off is do-not-disturb on.
pub fn gnome_banners_off(output: &str) -> Option<bool> {
    match output.trim() {
        "false" => Some(true),
        "true" => Some(false),
        _ => None,
    }
}

/// The value line of `reg query ...\Notifications\Settings /v
/// NOC_GLOBAL_SETTING_TOASTS_ENABLED`. `0x0` means notifications are off.
pub fn windows_toasts_disabled(output: &str) -> Option<bool> {
    let line = output.lines().find(|line| {
        line.trim_start()
            .starts_with("NOC_GLOBAL_SETTING_TOASTS_ENABLED")
    })?;
    let mut fields = line.split_whitespace().skip(1);
    if fields.next() != Some("REG_DWORD") {
        return None;
    }
    let value = fields.next()?;
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    let number = u32::from_str_radix(hex, 16).ok()?;
    Some(number == 0)
}

/// `~/Library/DoNotDisturb/DB/Assertions.json` on macOS 12 and later. A
/// Focus that is on leaves at least one assertion record.
pub fn macos_focus_on(assertions_json: &str) -> Option<bool> {
    let parsed: Value = serde_json::from_str(assertions_json).ok()?;
    let entries = parsed.get("data").and_then(Value::as_array);
    Some(entries.is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry
                .get("storeAssertionRecords")
                .and_then(Value::as_array)
                .is_some_and(|records| !records.is_empty())
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::{gnome_banners_off, macos_focus_on, windows_toasts_disabled};

    #[test]
    fn gnome_banners_off_reads_as_do_not_disturb() {
        assert_eq!(gnome_banners_off("false\n"), Some(true));
        assert_eq!(gnome_banners_off("true\n"), Some(false));
        assert_eq!(gnome_banners_off(""), None);
        assert_eq!(
            gnome_banners_off("No such schema “org.gnome.desktop.notifications”"),
            None
        );
    }

    #[test]
    fn the_windows_switch_is_read_from_the_reg_query_value_line() {
        let off = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings\r\n    NOC_GLOBAL_SETTING_TOASTS_ENABLED    REG_DWORD    0x0\r\n\r\n";
        assert_eq!(windows_toasts_disabled(off), Some(true));
        assert_eq!(
            windows_toasts_disabled(&off.replace("0x0", "0x1")),
            Some(false)
        );
        assert_eq!(
            windows_toasts_disabled(&off.replace("REG_DWORD", "REG_SZ")),
            None
        );
        assert_eq!(windows_toasts_disabled(&off.replace("0x0", "zero")), None);
        assert_eq!(
            windows_toasts_disabled(
                "ERROR: The system was unable to find the specified registry key or value."
            ),
            None
        );
    }

    #[test]
    fn a_macos_focus_shows_as_an_assertion_record() {
        let on = r#"{"data":[{"storeAssertionRecords":[{"assertionDetails":{"assertionDetailsModeIdentifier":"com.apple.donotdisturb.mode.default"},"assertionStartDateTimestamp":752000000}]}],"header":{"version":1}}"#;
        assert_eq!(macos_focus_on(on), Some(true));
        assert_eq!(
            macos_focus_on(r#"{"data":[{"storeAssertionRecords":[]}],"header":{}}"#),
            Some(false)
        );
        assert_eq!(macos_focus_on(r#"{"data":[{}]}"#), Some(false));
        assert_eq!(macos_focus_on("{}"), Some(false));
        assert_eq!(macos_focus_on("not json"), None);
    }
}
