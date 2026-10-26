//! What a check shows.
//!
//! A high-severity item is shown from the check that observed it, including
//! during quiet hours. Other results stay hidden during quiet hours. The
//! speech bubble is a short summary, not a conversation.

use crate::api::{https_origin, workspace_url, AlertItem};
use chrono::{DateTime, Timelike};
use chrono_tz::Tz;
use std::time::Duration;

pub const BUBBLE_VISIBLE_FOR: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub lines: Vec<String>,
    pub details_url: String,
}

impl Summary {
    pub fn body(&self) -> String {
        self.lines.join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentation {
    pub show_bubble: bool,
    pub show_os_notification: bool,
    pub summary: Summary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Unavailable,
    SignedOut,
}

pub fn calm_notice(kind: Notice, server_base: &str) -> Presentation {
    let lines = match kind {
        Notice::Unavailable => vec![
            "ORBIT is not available right now".to_string(),
            "Stardust will try again later".to_string(),
        ],
        Notice::SignedOut => vec![
            "Sign in to see ORBIT items".to_string(),
            "Use stardust sign-in in a terminal".to_string(),
        ],
    };
    Presentation {
        show_bubble: true,
        show_os_notification: false,
        summary: Summary {
            lines,
            details_url: https_origin(server_base).unwrap_or_default(),
        },
    }
}

pub fn present(
    items: &[AlertItem],
    quiet: bool,
    checked_local: DateTime<Tz>,
    force_bubble: bool,
    server_base: &str,
    fallback_path: &str,
) -> Presentation {
    let high = items.iter().filter(|item| item.shows_immediately()).count();
    let summary = summarize(items, high, checked_local, server_base, fallback_path);
    let mut show_bubble = !quiet || high > 0;
    let show_os_notification = high > 0;
    if force_bubble {
        show_bubble = true;
    }
    Presentation {
        show_bubble,
        show_os_notification,
        summary,
    }
}

fn summarize(
    items: &[AlertItem],
    high: usize,
    checked_local: DateTime<Tz>,
    server_base: &str,
    fallback_path: &str,
) -> Summary {
    let mut lines = vec![count_line(items.len(), high)];
    if let Some(title) = items.iter().find(|item| item.shows_immediately()) {
        lines.push(clean_title(&title.title));
    }
    lines.push(format!("Last checked time {}", clock(checked_local)));
    Summary {
        lines,
        details_url: details_url(items, server_base, fallback_path),
    }
}

fn count_line(total: usize, high: usize) -> String {
    if high == 1 {
        "1 high-severity item".to_string()
    } else if high > 1 {
        format!("{high} high-severity items")
    } else if total == 0 {
        "No new or changed items".to_string()
    } else if total == 1 {
        "1 new or changed item".to_string()
    } else {
        format!("{total} new or changed items")
    }
}

fn clean_title(title: &str) -> String {
    let mut cleaned: String = title
        .chars()
        .filter(|ch| !ch.is_control())
        .take(48)
        .collect();
    if title.chars().filter(|ch| !ch.is_control()).count() > 48 {
        cleaned.push_str("...");
    }
    cleaned
}

fn clock(local: DateTime<Tz>) -> String {
    format!("{:02}:{:02}", local.hour(), local.minute())
}

pub fn details_url(items: &[AlertItem], server_base: &str, fallback_path: &str) -> String {
    let preferred = items
        .iter()
        .filter(|item| item.shows_immediately())
        .find_map(|item| workspace_url(server_base, &item.workspace_path))
        .or_else(|| {
            items
                .iter()
                .find_map(|item| workspace_url(server_base, &item.workspace_path))
        })
        .or_else(|| workspace_url(server_base, fallback_path));
    preferred
        .or_else(|| https_origin(server_base))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{calm_notice, details_url, present, Notice, BUBBLE_VISIBLE_FOR};
    use crate::api::{AlertItem, Severity, DEFAULT_SERVER_BASE, PLACEHOLDER_ORIGIN};
    use crate::copy::find_disallowed_copy;
    use chrono::{TimeZone, Utc};
    use chrono_tz::UTC;

    fn item(id: &str, severity: Severity, workspace_path: &str) -> AlertItem {
        AlertItem {
            id: id.to_string(),
            severity,
            title: "Example connection finding with a very long title that should be cut"
                .to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 10, 21, 14, 5, 0).unwrap(),
            workspace_path: workspace_path.to_string(),
            immediate: severity == Severity::High,
        }
    }

    fn at_local() -> chrono::DateTime<chrono_tz::Tz> {
        UTC.with_ymd_and_hms(2026, 10, 21, 14, 5, 0).unwrap()
    }

    #[test]
    fn high_severity_is_shown_immediately_including_quiet_hours() {
        let high = item(
            "h",
            Severity::High,
            "/rust-preview/pqc/studio/app/control-room/needs-attention",
        );
        let shown = present(
            std::slice::from_ref(&high),
            true,
            at_local(),
            false,
            DEFAULT_SERVER_BASE,
            "",
        );
        assert!(shown.show_bubble);
        assert!(shown.show_os_notification);
        assert_eq!(shown.summary.lines[0], "1 high-severity item");
        assert!(shown
            .summary
            .lines
            .last()
            .unwrap()
            .starts_with("Last checked time 14:05"));
        assert_eq!(
            shown.summary.details_url,
            "https://helloorbit.app/rust-preview/pqc/studio/app/control-room/needs-attention"
        );
        assert!((3..=10).contains(&BUBBLE_VISIBLE_FOR.as_secs()));
    }

    #[test]
    fn quiet_hours_hide_other_results_unless_the_user_asked() {
        let medium = item("m", Severity::Medium, "/workspace/example");
        let quiet = present(
            std::slice::from_ref(&medium),
            true,
            at_local(),
            false,
            DEFAULT_SERVER_BASE,
            "",
        );
        assert!(!quiet.show_bubble);
        assert!(!quiet.show_os_notification);
        let asked = present(
            std::slice::from_ref(&medium),
            true,
            at_local(),
            true,
            DEFAULT_SERVER_BASE,
            "",
        );
        assert!(asked.show_bubble);
        assert!(!asked.show_os_notification);
        let open = present(
            std::slice::from_ref(&medium),
            false,
            at_local(),
            false,
            DEFAULT_SERVER_BASE,
            "",
        );
        assert!(open.show_bubble);
        assert!(!open.show_os_notification);
        assert_eq!(open.summary.lines[0], "1 new or changed item");
    }

    #[test]
    fn summary_copy_and_links_stay_plain() {
        let items = vec![
            item("m", Severity::Medium, "notes/local"),
            item(
                "h",
                Severity::High,
                "/rust-preview/pqc/studio/app/control-room/needs-attention",
            ),
        ];
        let shown = present(&items, false, at_local(), false, DEFAULT_SERVER_BASE, "");
        assert_eq!(shown.summary.lines[0], "1 high-severity item");
        assert!(shown.summary.lines[1].ends_with("..."));
        assert_eq!(
            shown.summary.details_url,
            "https://helloorbit.app/rust-preview/pqc/studio/app/control-room/needs-attention"
        );
        let empty = present(&[], false, at_local(), false, DEFAULT_SERVER_BASE, "");
        assert_eq!(empty.summary.lines[0], "No new or changed items");
        assert_eq!(empty.summary.details_url, PLACEHOLDER_ORIGIN);
        let findings = present(
            &[],
            false,
            at_local(),
            false,
            DEFAULT_SERVER_BASE,
            "/rust-preview/pqc/studio/app/findings",
        );
        assert_eq!(
            findings.summary.details_url,
            "https://helloorbit.app/rust-preview/pqc/studio/app/findings"
        );
        assert_eq!(
            details_url(
                &[item("x", Severity::Low, "javascript:open(1)")],
                DEFAULT_SERVER_BASE,
                ""
            ),
            PLACEHOLDER_ORIGIN
        );
        let calm = calm_notice(Notice::Unavailable, DEFAULT_SERVER_BASE);
        assert!(calm.show_bubble);
        assert!(!calm.show_os_notification);
        assert_eq!(calm.summary.details_url, PLACEHOLDER_ORIGIN);
        assert_eq!(
            calm_notice(Notice::SignedOut, "http://127.0.0.1:9")
                .summary
                .details_url,
            ""
        );
        for line in &shown.summary.lines {
            assert_eq!(find_disallowed_copy(line), None, "{line}");
            assert!(!line.to_ascii_lowercase().contains("chat"));
        }
    }
}
