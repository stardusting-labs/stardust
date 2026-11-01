//! Best-effort OS do-not-disturb probe, used only with the routing feature
//! on, before the bubble or the OS notification is shown.
//!
//! One short read when a check has something new to show: a `gsettings` or
//! `reg` child process that gets at most a second, or a small file on macOS.
//! The answer is kept for a few minutes, so alerts held during
//! do-not-disturb do not cost a read every check. Nothing runs in the
//! background and nothing is logged.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one answer is used.
const FRESH_FOR: Duration = Duration::from_secs(5 * 60);

static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// The OS do-not-disturb or focus switch looked on. Any failure reads as off.
pub fn active() -> bool {
    remembered(&LAST, Instant::now(), || probe().unwrap_or(false))
}

fn remembered(
    last: &Mutex<Option<(Instant, bool)>>,
    now: Instant,
    look: impl FnOnce() -> bool,
) -> bool {
    let mut last = last.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((at, on)) = *last {
        if now.saturating_duration_since(at) < FRESH_FOR {
            return on;
        }
    }
    let on = look();
    *last = Some((now, on));
    on
}

#[cfg(all(unix, not(target_os = "macos")))]
fn probe() -> Option<bool> {
    let mut command = std::process::Command::new("gsettings");
    command.args(["get", "org.gnome.desktop.notifications", "show-banners"]);
    let output = run_briefly(command, LIMIT)?;
    stardust_core::dnd::gnome_banners_off(&output)
}

#[cfg(windows)]
fn probe() -> Option<bool> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new("reg");
    command
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings",
            "/v",
            "NOC_GLOBAL_SETTING_TOASTS_ENABLED",
        ])
        .creation_flags(CREATE_NO_WINDOW);
    let output = run_briefly(command, LIMIT)?;
    stardust_core::dnd::windows_toasts_disabled(&output)
}

#[cfg(target_os = "macos")]
fn probe() -> Option<bool> {
    use std::io::Read;
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join("Library/DoNotDisturb/DB/Assertions.json");
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(256 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    stardust_core::dnd::macos_focus_on(&text)
}

#[cfg(not(any(unix, windows)))]
fn probe() -> Option<bool> {
    None
}

#[cfg(any(all(unix, not(target_os = "macos")), windows))]
const LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Run `command` and return what it printed, or `None` when it fails or
/// takes longer than `limit`. The child is polled, not waited on from
/// another thread; its output is a few bytes, so the pipe cannot fill.
#[cfg(any(all(unix, not(target_os = "macos")), windows))]
fn run_briefly(mut command: std::process::Command, limit: std::time::Duration) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut text = String::new();
    child
        .stdout
        .take()?
        .take(4 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_probe_answers_without_panicking() {
        let _ = super::active();
    }

    #[test]
    fn one_answer_is_kept_for_a_few_minutes() {
        use super::{remembered, FRESH_FOR};
        use std::cell::Cell;
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        let last = Mutex::new(None);
        let looks = Cell::new(0);
        let look = |on: bool| {
            looks.set(looks.get() + 1);
            on
        };
        let start = Instant::now();
        assert!(remembered(&last, start, || look(true)));
        assert!(remembered(&last, start + Duration::from_secs(60), || look(
            false
        )));
        assert_eq!(looks.get(), 1, "no second look inside the window");
        assert!(!remembered(&last, start + FRESH_FOR, || look(false)));
        assert_eq!(looks.get(), 2);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_slow_probe_is_stopped_and_a_quick_one_is_read() {
        use super::run_briefly;
        use std::process::Command;
        use std::time::{Duration, Instant};
        let started = Instant::now();
        let mut slow = Command::new("sleep");
        slow.arg("5");
        assert_eq!(run_briefly(slow, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        let mut quick = Command::new("printf");
        quick.arg("false");
        assert_eq!(
            run_briefly(quick, Duration::from_secs(2)).as_deref(),
            Some("false")
        );
        let failing = Command::new("false");
        assert_eq!(run_briefly(failing, Duration::from_secs(2)), None);
        assert_eq!(
            run_briefly(
                Command::new("stardust-no-such-tool"),
                Duration::from_secs(1)
            ),
            None
        );
    }
}
