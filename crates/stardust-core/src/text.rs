//! Tray and bubble wording.

use crate::config::Config;
use crate::i18n::{tr, Language, Msg};
use crate::integrations::integrations_label;
use crate::pet::{ObservedCheck, PetScale, PetSkin};
use crate::schedule::{format_interval, Schedule};
use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

/// The account item in the tray menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrayAccount {
    /// "Sign in...". Always this one with the launch sign-in window off.
    #[default]
    SignIn,
    SignOut,
    /// `--demo` has no account to sign in or out of.
    Hidden,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuText {
    pub tooltip: String,
    pub next_check: String,
    pub last_checked: String,
    pub schedule: String,
    pub character: String,
    pub integrations: String,
    pub pet_skin: PetSkin,
    pub pet_scale: PetScale,
    /// ORBIT connection line. Only with auto sign-in or the launch sign-in
    /// window on.
    pub status: Option<String>,
    pub account: TrayAccount,
    /// `(menu id, label)` for "Send test" on each enabled sink. Only with
    /// routing on.
    pub tests: Vec<(String, String)>,
}

const TEST_SEND_PREFIX: &str = "test-send:";

/// Menu id of "Send test" for integration `id`.
pub fn test_send_menu_id(id: &str) -> String {
    format!("{TEST_SEND_PREFIX}{id}")
}

/// The integration id a "Send test" menu id names.
pub fn test_send_target(menu_id: &str) -> Option<&str> {
    menu_id
        .strip_prefix(TEST_SEND_PREFIX)
        .filter(|id| crate::integrations::parse_id(id).as_deref() == Ok(*id))
}

/// Adds the connection line when auto sign-in or the launch sign-in window is
/// on.
pub fn with_connect_status_in(
    mut text: MenuText,
    config: &Config,
    phase: &crate::connect::FlowPhase,
    observed: ObservedCheck,
    signed_in: bool,
    language: Language,
) -> MenuText {
    if config.features().sign_in_on_timer() {
        text.status = Some(crate::connect::status_line(
            phase, observed, signed_in, language,
        ));
    }
    text
}

fn test_items(config: &Config, language: Language) -> Vec<(String, String)> {
    if !config.features().routing {
        return Vec::new();
    }
    config
        .integrations()
        .iter()
        .filter(|record| record.enabled && record.kind.routes_alerts())
        .map(|record| {
            (
                test_send_menu_id(&record.id),
                format!("{}: {}", tr(language, Msg::MenuSendTest), record.id),
            )
        })
        .collect()
}

pub fn connection_note(observed: ObservedCheck) -> Option<&'static str> {
    connection_note_in(observed, Language::English)
}

pub fn connection_note_in(observed: ObservedCheck, language: Language) -> Option<&'static str> {
    match observed {
        ObservedCheck::Unavailable => Some(tr(language, Msg::NoteUnavailable)),
        ObservedCheck::SignedOut => Some(tr(language, Msg::NoteSignedOut)),
        ObservedCheck::Waiting | ObservedCheck::Failed | ObservedCheck::Succeeded { .. } => None,
    }
}

pub fn with_connection_note(text: MenuText, observed: ObservedCheck) -> MenuText {
    with_connection_note_in(text, observed, Language::English)
}

pub fn with_connection_note_in(
    mut text: MenuText,
    observed: ObservedCheck,
    language: Language,
) -> MenuText {
    if let Some(note) = connection_note_in(observed, language) {
        text.tooltip = format!("{} · {note}", text.tooltip);
        text.last_checked = format!("{} · {note}", text.last_checked);
    }
    text
}

pub fn menu_text(config: &Config, now_utc: DateTime<Utc>) -> MenuText {
    menu_text_in(config, now_utc, Language::English)
}

/// Tray text in `language`.
pub fn menu_text_in(config: &Config, now_utc: DateTime<Utc>, language: Language) -> MenuText {
    let mut text = english_menu_text(config, now_utc);
    text.tests = test_items(config, language);
    text
}

fn english_menu_text(config: &Config, now_utc: DateTime<Utc>) -> MenuText {
    let schedule = config.schedule();
    let timezone = schedule.timezone();
    let now = now_utc.with_timezone(&timezone);
    let last = config
        .state()
        .last_checked
        .map(|instant| instant.with_timezone(&timezone));
    let anchor = config
        .state()
        .interval_anchor
        .map(|instant| instant.with_timezone(&timezone));
    let next = schedule.next_fire(now, anchor);
    let last_clock = match last {
        Some(time) => format_stamp(now, time),
        None => "—".to_string(),
    };
    let cadence = cadence_phrase(schedule);
    MenuText {
        tooltip: format!("{cadence} · last checked {last_clock}"),
        next_check: match next {
            Some(fire) => format!("Next check {}", format_stamp(now, fire.at)),
            None => "Next check —".to_string(),
        },
        last_checked: format!("Last checked time {last_clock}"),
        schedule: schedule.describe(),
        integrations: integrations_label(config.integrations()),
        character: if config.character_visible() {
            "Hide character".to_string()
        } else {
            "Show character".to_string()
        },
        pet_skin: config.pet_skin(),
        pet_scale: config.pet_scale(),
        status: None,
        account: TrayAccount::SignIn,
        tests: Vec::new(),
    }
}

fn cadence_phrase(schedule: &Schedule) -> String {
    match (
        schedule.interval_minutes(),
        schedule.clock_times().is_empty(),
    ) {
        (Some(minutes), true) => format!("Checked every {}", format_interval(minutes)),
        (Some(minutes), false) => format!(
            "Checked every {} and at set times",
            format_interval(minutes)
        ),
        (None, false) => "Checked at set times".to_string(),
        (None, true) => "Checked on a schedule".to_string(),
    }
}

fn format_stamp(now: DateTime<Tz>, time: DateTime<Tz>) -> String {
    if time.date_naive() == now.date_naive() {
        format!("{:02}:{:02}", time.hour(), time.minute())
    } else {
        format!(
            "{} {:02}:{:02}",
            weekday_name(time),
            time.hour(),
            time.minute()
        )
    }
}

fn weekday_name(time: DateTime<Tz>) -> &'static str {
    match time.weekday() {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

#[cfg(test)]
mod tests {
    use super::menu_text;
    use crate::config::{Config, FieldUpdate, ScheduleUpdate};
    use crate::copy::find_disallowed_copy;
    use chrono::TimeZone;
    use chrono::Utc;

    #[test]
    fn tray_text_names_the_interval_and_the_last_checked_time() {
        let mut config = Config::default_for("UTC").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        let update = ScheduleUpdate {
            interval: Some(FieldUpdate::Set(45)),
            clock_times: Some(vec!["18:00".to_string()]),
            ..ScheduleUpdate::default()
        };
        config.apply_schedule_update(&update, now).unwrap();
        let text = menu_text(&config, now);
        assert_eq!(
            text.tooltip,
            "Checked every 45 minutes and at set times · last checked —"
        );
        assert_eq!(text.last_checked, "Last checked time —");
        assert_eq!(text.next_check, "Next check 15:45");
        assert_eq!(text.character, "Show character");
        assert_eq!(text.integrations, "Integrations off");
        assert_eq!(text.pet_skin, crate::pet::PetSkin::Astronaut);
        assert_eq!(text.pet_scale, crate::pet::PetScale::X2);
        for value in [
            &text.tooltip,
            &text.next_check,
            &text.last_checked,
            &text.schedule,
            &text.character,
            &text.integrations,
        ] {
            assert_eq!(find_disallowed_copy(value), None, "{value}");
            assert!(!value.to_ascii_lowercase().contains("chat"));
        }
    }

    #[test]
    fn status_and_send_test_lines_appear_only_with_their_features_on() {
        use crate::config::Feature;
        use crate::connect::FlowPhase;
        use crate::i18n::Language;
        use crate::integrations::IntegrationKind;
        use crate::pet::ObservedCheck;
        let mut config = Config::default_for("UTC").unwrap();
        config
            .add_integration(IntegrationKind::DiscordWebhook, "chat")
            .unwrap();
        config
            .add_integration(IntegrationKind::GenericJson, "json")
            .unwrap();
        config.set_integration_enabled("chat", true).unwrap();
        config.set_integration_enabled("json", true).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        let waiting = FlowPhase::Waiting {
            user_code: "ABCD-EFGH".to_string(),
        };
        let off = super::with_connect_status_in(
            super::menu_text_in(&config, now, Language::English),
            &config,
            &waiting,
            ObservedCheck::Waiting,
            false,
            Language::English,
        );
        assert_eq!(off.status, None);
        assert!(off.tests.is_empty());
        config.set_feature(Feature::AutoSignIn, true);
        config.set_feature(Feature::Routing, true);
        let on = super::with_connect_status_in(
            super::menu_text_in(&config, now, Language::English),
            &config,
            &waiting,
            ObservedCheck::Waiting,
            false,
            Language::English,
        );
        assert_eq!(on.status.as_deref(), Some("ORBIT: Connecting... ABCD-EFGH"));
        assert_eq!(
            on.tests,
            vec![("test-send:chat".to_string(), "Send test: chat".to_string())]
        );
        assert_eq!(super::test_send_target("test-send:chat"), Some("chat"));
        assert_eq!(super::test_send_target("test-send:../x"), None);
        assert_eq!(super::test_send_target("interval-30"), None);
    }

    #[test]
    fn the_launch_window_flag_alone_adds_the_connection_line() {
        use crate::config::Feature;
        use crate::connect::FlowPhase;
        use crate::i18n::Language;
        use crate::pet::ObservedCheck;
        use crate::text::TrayAccount;
        let mut config = Config::default_for("UTC").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        let line = |config: &Config| {
            super::with_connect_status_in(
                super::menu_text_in(config, now, Language::English),
                config,
                &FlowPhase::Idle,
                ObservedCheck::Waiting,
                false,
                Language::English,
            )
        };
        assert_eq!(line(&config).status, None);
        assert_eq!(line(&config).account, TrayAccount::SignIn);
        config.set_feature(Feature::LaunchSignInWindow, true);
        assert_eq!(line(&config).status.as_deref(), Some("ORBIT: Signed out"));
    }
}
