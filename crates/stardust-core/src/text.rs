//! Tray and bubble wording.

use crate::config::Config;
use crate::i18n::{tr, Language, Msg};
use crate::integrations::integrations_label;
use crate::pet::{ObservedCheck, PetScale, PetSkin};
use crate::schedule::{format_interval, Schedule};
use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

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
///
/// The schedule and integrations lines are still English in both languages.
/// TODO(Unsu): Korean wording for those two lines.
pub fn menu_text_in(config: &Config, now_utc: DateTime<Utc>, language: Language) -> MenuText {
    let mut text = english_menu_text(config, now_utc);
    if language == Language::Korean {
        let last_clock = text
            .last_checked
            .strip_prefix("Last checked time ")
            .unwrap_or("—")
            .to_string();
        let next_clock = text
            .next_check
            .strip_prefix("Next check ")
            .unwrap_or("—")
            .to_string();
        text.tooltip = format!("Stardust · 마지막 확인 {last_clock}");
        text.last_checked = format!("마지막 확인 {last_clock}");
        text.next_check = format!("다음 확인 {next_clock}");
        text.character = if config.character_visible() {
            tr(language, Msg::HideCharacter).to_string()
        } else {
            tr(language, Msg::ShowCharacter).to_string()
        };
    }
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
    fn korean_tray_text_keeps_the_times_and_adds_the_note() {
        let config = Config::default_for("UTC").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        let text = super::with_connection_note_in(
            super::menu_text_in(&config, now, crate::i18n::Language::Korean),
            crate::pet::ObservedCheck::SignedOut,
            crate::i18n::Language::Korean,
        );
        assert_eq!(text.next_check, "다음 확인 16:00");
        assert_eq!(
            text.last_checked,
            "마지막 확인 — · ORBIT 항목을 보려면 로그인하세요"
        );
        assert_eq!(text.character, "캐릭터 보이기");
        assert_eq!(find_disallowed_copy(&text.tooltip), None);
    }
}
