//! The last delivery result per integration, kept in memory.
//!
//! Used only with the integrations panel on. Nothing here is written to disk
//! or holds a destination; entries are integration ids and a fixed outcome.

use crate::i18n::{tr, Language, Msg};
use crate::route::Tally;
use crate::sink::SinkError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Delivered,
    /// Busy or unreachable. A later check tries the alert again.
    RetryPending,
    /// The destination would not take it.
    Refused,
    /// 404 or 410. The integration was switched off.
    GoneDisabled,
    /// Hourly limit, per-check cap, or time budget. A later check sends it.
    RateLimited,
    HeldByQuietHours,
    /// No destination or secret to send with.
    NotReady,
    NeedsConsent,
    /// Something was waiting and no longer is: the alert left ORBIT's list.
    NothingWaiting,
    TestSending,
    TestSent,
    TestFailed,
}

impl Outcome {
    pub fn label(self, language: Language) -> &'static str {
        tr(
            language,
            match self {
                Self::Delivered => Msg::OutcomeDelivered,
                Self::RetryPending => Msg::OutcomeRetryPending,
                Self::Refused => Msg::OutcomeRefused,
                Self::GoneDisabled => Msg::OutcomeGoneDisabled,
                Self::RateLimited => Msg::OutcomeRateLimited,
                Self::HeldByQuietHours => Msg::OutcomeHeld,
                Self::NotReady => Msg::OutcomeNotReady,
                Self::NeedsConsent => Msg::OutcomeNeedsConsent,
                Self::NothingWaiting => Msg::OutcomeNothingWaiting,
                Self::TestSending => Msg::OutcomeTestSending,
                Self::TestSent => Msg::TestSent,
                Self::TestFailed => Msg::TestFailed,
            },
        )
    }

    /// Describes something still waiting to go out, so it stops being true
    /// once nothing is.
    fn waiting(self) -> bool {
        matches!(
            self,
            Self::RetryPending
                | Self::RateLimited
                | Self::HeldByQuietHours
                | Self::NotReady
                | Self::NeedsConsent
        )
    }

    /// Which outcome the tray tooltip shows first. Higher needs the user more.
    fn urgency(self) -> u8 {
        match self {
            Self::GoneDisabled => 9,
            Self::Refused => 8,
            Self::NeedsConsent => 7,
            Self::NotReady => 6,
            Self::RetryPending => 5,
            Self::TestFailed => 4,
            Self::RateLimited => 3,
            Self::HeldByQuietHours => 2,
            Self::TestSending | Self::TestSent | Self::Delivered | Self::NothingWaiting => 1,
        }
    }
}

/// The destination says it no longer exists. Only these switch an integration
/// off: a 401, 403, redirect, or DNS blocker can be temporary or local.
pub fn is_gone_for_good(err: &SinkError) -> bool {
    matches!(err, SinkError::Rejected(404 | 410))
}

/// The outcome of one sink in one check, from the router's report. `None`
/// means nothing happened worth showing, such as only filtered alerts.
pub fn outcome_of(tally: Tally, error: Option<&SinkError>) -> Option<Outcome> {
    if let Some(err) = error {
        return Some(if is_gone_for_good(err) {
            Outcome::GoneDisabled
        } else if err.retry() {
            Outcome::RetryPending
        } else {
            Outcome::Refused
        });
    }
    if tally.limited > 0 {
        Some(Outcome::RateLimited)
    } else if tally.held > 0 {
        Some(Outcome::HeldByQuietHours)
    } else if tally.delivered > 0 {
        Some(Outcome::Delivered)
    } else {
        None
    }
}

/// The result of "Send test" as the panel shows it.
pub fn outcome_of_test(result: &Result<String, SinkError>) -> Outcome {
    match result {
        Ok(_) => Outcome::TestSent,
        Err(SinkError::NeedsConsent) => Outcome::NeedsConsent,
        Err(err) if is_gone_for_good(err) => Outcome::GoneDisabled,
        Err(_) => Outcome::TestFailed,
    }
}

/// Last outcome per integration id. At most one entry per id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeliveryBoard {
    entries: Vec<(String, Outcome)>,
}

impl DeliveryBoard {
    pub fn get(&self, id: &str) -> Option<Outcome> {
        self.entries
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, outcome)| *outcome)
    }

    pub fn record(&mut self, id: &str, outcome: Outcome) {
        let at = self.entries.iter().position(|(name, _)| name == id);
        match (at, outcome) {
            // "Nothing waiting" only ever corrects a stale waiting state: it
            // must not hide the last real result.
            (Some(at), Outcome::NothingWaiting) => {
                if self.entries[at].1.waiting() {
                    self.entries[at].1 = Outcome::NothingWaiting;
                }
            }
            (None, Outcome::NothingWaiting) => {}
            (Some(at), other) => self.entries[at].1 = other,
            (None, other) => self.entries.push((id.to_string(), other)),
        }
    }

    pub fn clear(&mut self, id: &str) {
        self.entries.retain(|(name, _)| name != id);
    }

    /// The one line the tray tooltip shows: the outcome that needs the user
    /// most, and how many others there are. Windows cuts tooltips short, so
    /// it does not list them all. `ids` limits it to integrations that still
    /// exist.
    pub fn tooltip_note(&self, ids: &[&str], language: Language) -> Option<String> {
        let known: Vec<&(String, Outcome)> = self
            .entries
            .iter()
            .filter(|(name, _)| ids.contains(&name.as_str()))
            .collect();
        let worst = known.iter().max_by_key(|(_, outcome)| outcome.urgency())?;
        let line = format!("{}: {}", worst.0, worst.1.label(language));
        Some(match known.len() - 1 {
            0 => line,
            more => format!("{line} (+{more} more)"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{is_gone_for_good, outcome_of, outcome_of_test, DeliveryBoard, Outcome};
    use crate::copy::find_disallowed_copy;
    use crate::i18n::Language;
    use crate::route::Tally;
    use crate::sink::SinkError;

    fn tally(delivered: u32, held: u32, limited: u32, failed: u32) -> Tally {
        Tally {
            delivered,
            held,
            limited,
            failed,
            ..Tally::default()
        }
    }

    #[test]
    fn only_404_and_410_count_as_gone() {
        for gone in [404, 410] {
            assert!(is_gone_for_good(&SinkError::Rejected(gone)));
        }
        for kept in [
            SinkError::Rejected(301),
            SinkError::Rejected(400),
            SinkError::Rejected(401),
            SinkError::Rejected(403),
            SinkError::Blocked,
            SinkError::Busy(503),
            SinkError::Unreachable,
            SinkError::Refused,
            SinkError::Missing,
        ] {
            assert!(!is_gone_for_good(&kept), "{kept:?}");
        }
    }

    #[test]
    fn a_tally_and_its_error_become_one_outcome() {
        let failed = tally(0, 0, 1, 1);
        assert_eq!(
            outcome_of(failed, Some(&SinkError::Rejected(410))),
            Some(Outcome::GoneDisabled)
        );
        assert_eq!(
            outcome_of(failed, Some(&SinkError::Busy(500))),
            Some(Outcome::RetryPending)
        );
        assert_eq!(
            outcome_of(failed, Some(&SinkError::Unreachable)),
            Some(Outcome::RetryPending)
        );
        assert_eq!(
            outcome_of(failed, Some(&SinkError::Rejected(400))),
            Some(Outcome::Refused)
        );
        assert_eq!(
            outcome_of(failed, Some(&SinkError::Blocked)),
            Some(Outcome::Refused),
            "a DNS blocker is reported, and nothing is switched off"
        );
        assert_eq!(
            outcome_of(tally(1, 0, 2, 0), None),
            Some(Outcome::RateLimited)
        );
        assert_eq!(
            outcome_of(tally(1, 2, 0, 0), None),
            Some(Outcome::HeldByQuietHours),
            "an alert is still waiting even though another went out"
        );
        assert_eq!(
            outcome_of(tally(3, 0, 0, 0), None),
            Some(Outcome::Delivered)
        );
        assert_eq!(outcome_of(Tally::default(), None), None);
        let filtered = Tally {
            filtered: 2,
            ..Tally::default()
        };
        assert_eq!(outcome_of(filtered, None), None);
    }

    #[test]
    fn test_sends_report_their_own_result() {
        assert_eq!(
            outcome_of_test(&Ok("hooks.example".to_string())),
            Outcome::TestSent
        );
        assert_eq!(
            outcome_of_test(&Err(SinkError::Busy(500))),
            Outcome::TestFailed
        );
        assert_eq!(
            outcome_of_test(&Err(SinkError::Rejected(404))),
            Outcome::GoneDisabled
        );
        assert_eq!(
            outcome_of_test(&Err(SinkError::NeedsConsent)),
            Outcome::NeedsConsent
        );
    }

    #[test]
    fn the_board_keeps_one_result_per_id_and_corrects_only_stale_waiting() {
        let mut board = DeliveryBoard::default();
        assert_eq!(board.get("ops"), None);
        board.record("ops", Outcome::NothingWaiting);
        assert_eq!(board.get("ops"), None, "no result yet stays no result");
        board.record("ops", Outcome::RetryPending);
        board.record("ops", Outcome::RetryPending);
        assert_eq!(board.entries.len(), 1);
        board.record("ops", Outcome::NothingWaiting);
        assert_eq!(board.get("ops"), Some(Outcome::NothingWaiting));
        board.record("ops", Outcome::Delivered);
        board.record("ops", Outcome::NothingWaiting);
        assert_eq!(
            board.get("ops"),
            Some(Outcome::Delivered),
            "the last real result stays"
        );
        board.clear("ops");
        assert_eq!(board.get("ops"), None);
    }

    #[test]
    fn the_tooltip_leads_with_what_needs_the_user_and_skips_removed_ids() {
        let mut board = DeliveryBoard::default();
        assert_eq!(board.tooltip_note(&["ops"], Language::English), None);
        board.record("ops", Outcome::Delivered);
        assert_eq!(
            board.tooltip_note(&["ops"], Language::English).as_deref(),
            Some("ops: Delivered")
        );
        board.record("pd", Outcome::RetryPending);
        board.record("old", Outcome::GoneDisabled);
        assert_eq!(
            board
                .tooltip_note(&["ops", "pd"], Language::English)
                .as_deref(),
            Some("pd: Retry pending (+1 more)")
        );
        assert_eq!(
            board
                .tooltip_note(&["ops", "pd", "old"], Language::English)
                .as_deref(),
            Some("old: Turned off: destination gone (+2 more)")
        );
    }

    #[test]
    fn every_outcome_has_plain_short_wording() {
        for outcome in [
            Outcome::Delivered,
            Outcome::RetryPending,
            Outcome::Refused,
            Outcome::GoneDisabled,
            Outcome::RateLimited,
            Outcome::HeldByQuietHours,
            Outcome::NotReady,
            Outcome::NeedsConsent,
            Outcome::NothingWaiting,
            Outcome::TestSending,
            Outcome::TestSent,
            Outcome::TestFailed,
        ] {
            let label = outcome.label(Language::English);
            assert!(label.is_ascii() && label.len() <= 28, "{label}");
            assert_eq!(find_disallowed_copy(label), None, "{label}");
        }
    }
}
