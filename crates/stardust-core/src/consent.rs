//! What an integration sends, and the user's recorded yes to it.
//!
//! Used only with the integrations panel on. The wording is per destination
//! because the payloads differ: the tests in [`crate::sink`] compare it with
//! what each sink really posts. Consent is tied to the destination host, so a
//! new host asks again.

use crate::integrations::{IntegrationKind, IntegrationRecord};
use crate::sink::PAGERDUTY_DEFAULT_HOST;
use serde::{Deserialize, Serialize};

/// Raise this when the wording changes in a way people should see again.
pub const CONSENT_VERSION: u32 = 1;

/// A recorded yes: which host, and which version of the wording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub host: String,
    pub version: u32,
}

/// Where this integration's alerts would go, host only. PagerDuty without a
/// saved URL uses its US endpoint. MCP kinds take no alerts, so they have no
/// destination here.
pub fn destination_host(record: &IntegrationRecord) -> Option<String> {
    match record.kind {
        IntegrationKind::PagerDuty => record
            .host
            .clone()
            .or_else(|| Some(PAGERDUTY_DEFAULT_HOST.to_string())),
        IntegrationKind::SlackIncoming
        | IntegrationKind::TeamsIncoming
        | IntegrationKind::DiscordWebhook
        | IntegrationKind::GenericJson => record.host.clone(),
        IntegrationKind::OrbitMcp | IntegrationKind::SlackMcp => None,
    }
}

/// The recorded yes covers this wording and the current destination.
pub fn has_valid_consent(record: &IntegrationRecord) -> bool {
    let Some(consent) = &record.consent else {
        return false;
    };
    consent.version == CONSENT_VERSION
        && destination_host(record).as_deref() == Some(consent.host.as_str())
}

/// This integration would send alerts and has no valid yes for them.
pub fn needs_consent(record: &IntegrationRecord) -> bool {
    record.kind.delivers_alerts() && !has_valid_consent(record)
}

/// Exactly what leaves this computer for each alert of this kind. `None` for
/// the kinds no alert reaches.
pub fn what_is_sent(kind: IntegrationKind) -> Option<&'static str> {
    match kind {
        IntegrationKind::SlackIncoming
        | IntegrationKind::TeamsIncoming
        | IntegrationKind::DiscordWebhook => {
            Some("Alert title, severity, time, and ORBIT link. Nothing else.")
        }
        IntegrationKind::PagerDuty => Some(
            "Alert title, severity, time, ORBIT link, alert ID, and ORBIT host. \
             Your routing key goes with each post so PagerDuty accepts it. Nothing else.",
        ),
        IntegrationKind::GenericJson => Some(
            "Alert title, severity, alert and check times, ORBIT link, alert ID, event ID, \
             placeholder tenant and workspace IDs, and check interval. Each post is signed \
             with your signing secret, which is not sent. Nothing else.",
        ),
        IntegrationKind::OrbitMcp | IntegrationKind::SlackMcp => None,
    }
}

/// Break `text` into lines of at most `width` characters. Words stay whole
/// when they fit; a longer one, such as a host name, is cut across lines so
/// none of it is lost.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        let line_len = line.chars().count();
        if line_len > 0 && line_len + 1 + word_len <= width {
            line.push(' ');
            line.push_str(word);
            continue;
        }
        if line_len > 0 {
            lines.push(std::mem::take(&mut line));
        }
        let chars: Vec<char> = word.chars().collect();
        let mut chunks = chars.chunks(width).peekable();
        while let Some(chunk) = chunks.next() {
            let piece: String = chunk.iter().collect();
            if chunks.peek().is_some() {
                lines.push(piece);
            } else {
                line = piece;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{
        destination_host, has_valid_consent, needs_consent, what_is_sent, wrap, Consent,
        CONSENT_VERSION,
    };
    use crate::copy::find_disallowed_copy;
    use crate::integrations::{IntegrationKind, IntegrationRecord};

    fn record(kind: IntegrationKind, host: Option<&str>) -> IntegrationRecord {
        let mut record = IntegrationRecord::new(kind, "ops").unwrap();
        record.host = host.map(str::to_string);
        record
    }

    #[test]
    fn consent_is_tied_to_the_host_and_the_wording_version() {
        let mut slack = record(IntegrationKind::SlackIncoming, Some("hooks.slack.com"));
        assert!(needs_consent(&slack));
        slack.consent = Some(Consent {
            host: "hooks.slack.com".to_string(),
            version: CONSENT_VERSION,
        });
        assert!(has_valid_consent(&slack));
        assert!(!needs_consent(&slack));
        slack.host = Some("hooks.slack-gov.com".to_string());
        assert!(needs_consent(&slack), "a new host asks again");
        slack.host = Some("hooks.slack.com".to_string());
        slack.consent = Some(Consent {
            host: "hooks.slack.com".to_string(),
            version: CONSENT_VERSION + 1,
        });
        assert!(needs_consent(&slack), "another wording asks again");
    }

    #[test]
    fn pagerduty_defaults_to_its_us_host_and_mcp_needs_no_consent() {
        let pager = record(IntegrationKind::PagerDuty, None);
        assert_eq!(
            destination_host(&pager).as_deref(),
            Some("events.pagerduty.com")
        );
        for kind in [IntegrationKind::OrbitMcp, IntegrationKind::SlackMcp] {
            let mcp = record(kind, Some("mcp.example"));
            assert_eq!(destination_host(&mcp), None);
            assert!(!needs_consent(&mcp));
            assert_eq!(what_is_sent(kind), None);
        }
        let unsaved = record(IntegrationKind::SlackIncoming, None);
        assert_eq!(destination_host(&unsaved), None);
    }

    #[test]
    fn each_destination_states_its_own_list_in_plain_english() {
        for kind in [
            IntegrationKind::SlackIncoming,
            IntegrationKind::TeamsIncoming,
            IntegrationKind::DiscordWebhook,
        ] {
            let text = what_is_sent(kind).unwrap();
            assert_eq!(
                text,
                "Alert title, severity, time, and ORBIT link. Nothing else."
            );
        }
        let pager = what_is_sent(IntegrationKind::PagerDuty).unwrap();
        assert!(pager.contains("alert ID, and ORBIT host"), "{pager}");
        assert!(pager.contains("routing key"), "{pager}");
        let json = what_is_sent(IntegrationKind::GenericJson).unwrap();
        for part in [
            "alert ID",
            "event ID",
            "placeholder tenant and workspace IDs",
            "check interval",
            "not sent",
        ] {
            assert!(json.contains(part), "{part}: {json}");
        }
        for kind in [IntegrationKind::PagerDuty, IntegrationKind::GenericJson] {
            let text = what_is_sent(kind).unwrap();
            assert!(text.is_ascii(), "{text}");
            assert_eq!(find_disallowed_copy(text), None, "{text}");
            assert!(text.ends_with("Nothing else."), "{text}");
        }
    }

    #[test]
    fn wrapping_keeps_every_character_and_stays_inside_the_width() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("", 10), Vec::<String>::new());
        let host = format!("{}.example", "a".repeat(60));
        let lines = wrap(&host, 25);
        assert!(lines.iter().all(|line| line.chars().count() <= 25));
        assert_eq!(lines.concat(), host);
        let mixed = wrap(&format!("to {host} now"), 25);
        assert!(mixed.iter().all(|line| line.chars().count() <= 25));
        assert_eq!(mixed.first().map(String::as_str), Some("to"));
        assert_eq!(
            mixed.last().map(String::as_str),
            Some("aaaaaaaaaa.example now")
        );
    }
}
