//! Launch sign-in window model.
//!
//! The window is drawn by the app. This module owns its states, rows, wording,
//! keys, and clicks, so the behaviour is testable without a display. It is used
//! only with the `launch-sign-in-window` feature on.
//!
//! The device flow itself stays on the check timer. This model only shows where
//! the flow stands, and holds a user code and two addresses, none of them a
//! secret. The device code and the tokens never reach it.

use crate::api::OrbitError;
use crate::connect::IssuedCode;
use crate::i18n::{tr, Language, Msg};
use crate::settings::Key;
use crate::token::{TokenError, TokenStore};
use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};

/// Widest line, in bitmap cells. Letters are drawn at a fixed width and cut
/// off at the column limit, so every line is made to fit.
pub const COLUMNS: usize = 52;
/// Lines in the tallest state with an ordinary host. See
/// [`SignInModel::max_lines`].
const BASE_LINES: usize = 20;
/// Keys and clicks wait this long after the window opens or its state changes,
/// so a held key, a stray Enter at login, or the second half of a double click
/// does not act on a row the user has not seen yet.
const INPUT_GUARD: Duration = Duration::from_millis(500);
const SIGNED_IN_CLOSES_AFTER: Duration = Duration::from_secs(2);
/// An address longer than this many lines is not shown. The Open row says so.
const MAX_ADDRESS_LINES: usize = 3;

/// What the keychain holds at launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRead {
    /// A live token, or one that can be refreshed. `until` is when the record
    /// stops being usable; `None` is no known end.
    Usable {
        until: Option<DateTime<Utc>>,
    },
    Missing,
    /// The keychain could not be read. A corrupt record is not this: a new
    /// sign-in overwrites it, so it counts as [`SessionRead::Missing`].
    Unreadable,
}

pub fn read_session(store: &dyn TokenStore, now: DateTime<Utc>) -> SessionRead {
    match store.load(now) {
        Ok(Some(token)) => SessionRead::Usable {
            until: token.usable_until(),
        },
        Err(TokenError::Keychain) => SessionRead::Unreadable,
        Ok(None) | Err(_) => SessionRead::Missing,
    }
}

/// What a launch opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchWindow {
    None,
    SignIn,
    KeychainUnreadable,
}

/// The launch rule. `--headless` and a machine with no display never reach it.
pub fn opens_at_launch(flag: bool, demo: bool, session: SessionRead) -> LaunchWindow {
    if !flag || demo {
        return LaunchWindow::None;
    }
    match session {
        SessionRead::Usable { .. } => LaunchWindow::None,
        SessionRead::Missing => LaunchWindow::SignIn,
        SessionRead::Unreadable => LaunchWindow::KeychainUnreadable,
    }
}

/// Why a sign-in stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInEnd {
    Denied,
    Expired,
    Offline,
    Unavailable,
    KeychainFailed,
}

impl SignInEnd {
    pub fn from_error(err: &OrbitError) -> Self {
        match err {
            OrbitError::Denied => Self::Denied,
            OrbitError::Expired => Self::Expired,
            OrbitError::Unreachable => Self::Offline,
            OrbitError::FeatureOff
            | OrbitError::Unauthorized
            | OrbitError::Response
            | OrbitError::Refused
            | OrbitError::ServerBase => Self::Unavailable,
        }
    }

    pub fn message(self, language: Language) -> &'static str {
        tr(
            language,
            match self {
                Self::Denied => Msg::SignInDenied,
                Self::Expired => Msg::SignInExpired,
                Self::Offline => Msg::SignInOffline,
                Self::Unavailable => Msg::SignInUnavailable,
                Self::KeychainFailed => Msg::SignInKeychainFailed,
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInRow {
    Open,
    Copy,
    Cancel,
    TryAgain,
    NewCode,
    NotNow,
    Close,
}

/// A short remark under the status lines, after the user chose a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Note {
    BrowserFailed,
    Copied,
    CopyFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    Requesting,
    Waiting {
        code: IssuedCode,
        expires_at: Instant,
        reconnecting: bool,
        note: Option<Note>,
    },
    SignedIn {
        closes_at: Instant,
    },
    Ended(SignInEnd),
    KeychainUnreadable,
}

/// What a key or a click asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInAction {
    None,
    /// Focus moved; redraw only.
    Moved,
    /// Open this page. It is https on the configured host.
    Open(String),
    Copy(String),
    /// Ask for a new code.
    Restart,
    /// Close the window, and stop any sign-in still running.
    Close,
}

/// What a timer tick did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    Idle,
    /// Redraw this window. The tray is not touched.
    Changed,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInLineKind {
    Title,
    Info,
    /// Drawn large, across this line and the [`SignInLineKind::CodeGap`] below.
    Code,
    CodeGap,
    Row(SignInRow),
    Hint,
    Blank,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInLine {
    pub kind: SignInLineKind,
    pub text: String,
    pub focused: bool,
}

impl SignInLine {
    fn new(kind: SignInLineKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            focused: false,
        }
    }

    fn blank() -> Self {
        Self::new(SignInLineKind::Blank, "")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInModel {
    stage: Stage,
    focus: usize,
    /// The ORBIT host, without a scheme.
    host: String,
    input_after: Instant,
}

impl SignInModel {
    /// The window opens asking ORBIT for a code.
    pub fn requesting(host: impl Into<String>, now: Instant) -> Self {
        Self {
            stage: Stage::Requesting,
            focus: 0,
            host: host.into(),
            input_after: now + INPUT_GUARD,
        }
    }

    /// The window opens saying the keychain could not be read.
    pub fn keychain_unreadable(host: impl Into<String>, now: Instant) -> Self {
        Self {
            stage: Stage::KeychainUnreadable,
            focus: 0,
            host: host.into(),
            input_after: now + INPUT_GUARD,
        }
    }

    fn enter(&mut self, stage: Stage, now: Instant) {
        self.stage = stage;
        self.focus = 0;
        self.input_after = now + INPUT_GUARD;
    }

    /// Try again and Get a new code always ask for a new code.
    pub fn start_over(&mut self, now: Instant) {
        self.enter(Stage::Requesting, now);
    }

    /// Once the token is saved the window stays signed in: a code or an ending
    /// that arrives after that belongs to an attempt that no longer matters.
    fn is_signed_in(&self) -> bool {
        matches!(self.stage, Stage::SignedIn { .. })
    }

    pub fn on_code(&mut self, code: IssuedCode, lifetime: Duration, now: Instant) {
        if self.is_signed_in() {
            return;
        }
        self.enter(
            Stage::Waiting {
                code,
                expires_at: now + lifetime,
                reconnecting: false,
                note: None,
            },
            now,
        );
    }

    pub fn on_end(&mut self, end: SignInEnd, now: Instant) {
        // The countdown may have got there first. Keep the focus where it is.
        if self.stage != Stage::Ended(end) && !self.is_signed_in() {
            self.enter(Stage::Ended(end), now);
        }
    }

    pub fn on_signed_in(&mut self, now: Instant) {
        self.enter(
            Stage::SignedIn {
                closes_at: now + SIGNED_IN_CLOSES_AFTER,
            },
            now,
        );
    }

    /// The last poll reached ORBIT, or did not. Only for the code on screen;
    /// returns whether anything changed.
    pub fn on_connection(&mut self, user_code: &str, reachable: bool) -> bool {
        if let Stage::Waiting {
            code, reconnecting, ..
        } = &mut self.stage
        {
            if code.user_code == user_code && *reconnecting == reachable {
                *reconnecting = !reachable;
                return true;
            }
        }
        false
    }

    pub fn is_keychain_unreadable(&self) -> bool {
        self.stage == Stage::KeychainUnreadable
    }

    pub fn note_browser_failed(&mut self) {
        self.set_note(Note::BrowserFailed);
    }

    pub fn note_copied(&mut self, copied: bool) {
        self.set_note(if copied {
            Note::Copied
        } else {
            Note::CopyFailed
        });
    }

    fn set_note(&mut self, value: Note) {
        if let Stage::Waiting { note, .. } = &mut self.stage {
            *note = Some(value);
        }
    }

    /// Moves to Expired when the countdown ends, without waiting for the check
    /// timer, which can be busy for a long time.
    pub fn tick(&mut self, now: Instant) -> Tick {
        match &self.stage {
            Stage::Waiting { expires_at, .. } if now >= *expires_at => {
                self.enter(Stage::Ended(SignInEnd::Expired), now);
                Tick::Changed
            }
            Stage::Waiting { .. } => Tick::Changed,
            Stage::SignedIn { closes_at } if now >= *closes_at => Tick::Close,
            _ => Tick::Idle,
        }
    }

    /// When [`SignInModel::tick`] has something to do. Only while a countdown
    /// or the closing message is on screen.
    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        match &self.stage {
            Stage::Waiting { expires_at, .. } => {
                let left = expires_at.saturating_duration_since(now);
                let fraction = Duration::from_nanos(u64::from(left.subsec_nanos()));
                Some(
                    now + if fraction.is_zero() {
                        Duration::from_secs(1).min(left)
                    } else {
                        fraction
                    },
                )
            }
            Stage::SignedIn { closes_at } => Some(*closes_at),
            _ => None,
        }
    }

    /// Rows for this state. Open is there only when a page may be opened.
    pub fn rows(&self) -> Vec<SignInRow> {
        match &self.stage {
            Stage::Requesting => vec![SignInRow::Cancel],
            Stage::Waiting { code, .. } => {
                let mut rows = Vec::with_capacity(3);
                if code.open_url.is_some() {
                    rows.push(SignInRow::Open);
                }
                rows.push(SignInRow::Copy);
                rows.push(SignInRow::Cancel);
                rows
            }
            Stage::SignedIn { .. } => vec![SignInRow::Close],
            Stage::Ended(SignInEnd::Expired) => vec![SignInRow::NewCode, SignInRow::NotNow],
            Stage::Ended(_) | Stage::KeychainUnreadable => {
                vec![SignInRow::TryAgain, SignInRow::NotNow]
            }
        }
    }

    /// Nothing is focused while asking for a code. Cancel is the only row then,
    /// and asking can take ten seconds, so a second Enter from someone who sees
    /// nothing happening must not end the attempt. Esc and a click still do.
    fn has_focus(&self) -> bool {
        !matches!(self.stage, Stage::Requesting)
    }

    pub fn focused(&self) -> Option<SignInRow> {
        if !self.has_focus() {
            return None;
        }
        self.rows().get(self.focus).copied()
    }

    /// The most lines this window can draw, so it is sized once and does not
    /// jump between states. A host too long for its usual two lines adds to it.
    pub fn max_lines(&self) -> usize {
        BASE_LINES + wrap_words(&self.host, COLUMNS).len().saturating_sub(2)
    }

    /// `repeat` keys are ignored by the caller, which has them from the
    /// platform. Everything else goes through here.
    pub fn handle_key(&mut self, key: Key, now: Instant) -> SignInAction {
        if now < self.input_after {
            return SignInAction::None;
        }
        let count = self.rows().len();
        match key {
            Key::Escape => SignInAction::Close,
            Key::Activate => match self.focused() {
                Some(row) => self.activate(row),
                None => SignInAction::None,
            },
            Key::Left | Key::Right => SignInAction::None,
            _ if !self.has_focus() => SignInAction::None,
            Key::Up | Key::BackTab => {
                self.focus = (self.focus + count - 1) % count;
                SignInAction::Moved
            }
            Key::Down | Key::Tab => {
                self.focus = (self.focus + 1) % count;
                SignInAction::Moved
            }
            Key::Home => {
                self.focus = 0;
                SignInAction::Moved
            }
            Key::End => {
                self.focus = count - 1;
                SignInAction::Moved
            }
        }
    }

    /// A click on line `line_index` of [`SignInModel::lines`] focuses that row
    /// and chooses it. Other lines do nothing.
    pub fn click(&mut self, line_index: usize, now: Instant) -> SignInAction {
        if now < self.input_after {
            return SignInAction::None;
        }
        let lines = self.lines(now, Language::English);
        let Some(SignInLineKind::Row(row)) = lines.get(line_index).map(|line| line.kind) else {
            return SignInAction::None;
        };
        if let Some(at) = self.rows().iter().position(|candidate| *candidate == row) {
            self.focus = at;
        }
        self.activate(row)
    }

    fn activate(&self, row: SignInRow) -> SignInAction {
        match row {
            SignInRow::Open => match &self.stage {
                Stage::Waiting { code, .. } => code
                    .open_url
                    .clone()
                    .map_or(SignInAction::None, SignInAction::Open),
                _ => SignInAction::None,
            },
            SignInRow::Copy => match &self.stage {
                Stage::Waiting { code, .. } => SignInAction::Copy(code.user_code.clone()),
                _ => SignInAction::None,
            },
            SignInRow::TryAgain | SignInRow::NewCode => SignInAction::Restart,
            SignInRow::Cancel | SignInRow::NotNow | SignInRow::Close => SignInAction::Close,
        }
    }

    pub fn lines(&self, now: Instant, language: Language) -> Vec<SignInLine> {
        let mut lines = vec![
            SignInLine::new(SignInLineKind::Title, tr(language, Msg::SignInTitle)),
            SignInLine::blank(),
        ];
        match &self.stage {
            Stage::Requesting => info(&mut lines, tr(language, Msg::SignInRequesting)),
            Stage::Waiting {
                code,
                expires_at,
                reconnecting,
                note,
            } => {
                info(&mut lines, tr(language, Msg::WaitingForApproval));
                lines.push(SignInLine::new(SignInLineKind::Code, &code.user_code));
                lines.push(SignInLine::new(SignInLineKind::CodeGap, ""));
                address_lines(&mut lines, code, &self.host, language);
                lines.push(SignInLine::blank());
                info(&mut lines, tr(language, Msg::SignInCheckYouStarted));
                lines.push(SignInLine::blank());
                info(
                    &mut lines,
                    &format!(
                        "{} {}",
                        tr(language, Msg::SignInExpiresIn),
                        countdown(expires_at.saturating_duration_since(now))
                    ),
                );
                info(
                    &mut lines,
                    tr(
                        language,
                        if *reconnecting {
                            Msg::SignInReconnecting
                        } else {
                            Msg::SignInWaiting
                        },
                    ),
                );
                // Always one line, so the rows stay put when a note appears.
                lines.push(match note {
                    Some(note) => SignInLine::new(
                        SignInLineKind::Info,
                        note_text(*note, drawn_address(code).is_some(), language),
                    ),
                    None => SignInLine::blank(),
                });
            }
            Stage::SignedIn { .. } => {
                info(
                    &mut lines,
                    &format!("{} {}", tr(language, Msg::SignInSignedInTo), self.host),
                );
            }
            Stage::Ended(end) => info(&mut lines, end.message(language)),
            Stage::KeychainUnreadable => {
                info(&mut lines, tr(language, Msg::SignInKeychainUnreadable));
            }
        }
        lines.push(SignInLine::blank());
        for (index, row) in self.rows().into_iter().enumerate() {
            let focused = self.has_focus() && index == self.focus;
            let marker = if focused { "> " } else { "  " };
            lines.push(SignInLine {
                kind: SignInLineKind::Row(row),
                text: format!("{marker}{}", row_text(row, language)),
                focused,
            });
        }
        lines.push(SignInLine::blank());
        lines.push(SignInLine::new(
            SignInLineKind::Hint,
            tr(
                language,
                match self.stage {
                    Stage::Requesting => Msg::SignInKeysRequesting,
                    Stage::Waiting { .. } => Msg::SignInKeysCancel,
                    _ => Msg::SignInKeysClose,
                },
            ),
        ));
        lines
    }

    /// The window title, which the OS exposes to assistive technology. It
    /// carries the state and the code. The countdown stays out, so a screen
    /// reader is not interrupted every second.
    pub fn title(&self, language: Language) -> String {
        let name = tr(language, Msg::SignInWindowName);
        match &self.stage {
            Stage::Requesting => format!("{name} - {}", tr(language, Msg::SignInRequesting)),
            Stage::Waiting {
                code, reconnecting, ..
            } => {
                let enter = tr(language, Msg::SignInEnterCode);
                if *reconnecting {
                    format!(
                        "{name} - {enter} {} - {}",
                        code.user_code,
                        tr(language, Msg::SignInReconnecting)
                    )
                } else {
                    format!("{name} - {enter} {}", code.user_code)
                }
            }
            Stage::SignedIn { .. } => {
                format!(
                    "{name} - {} {}",
                    tr(language, Msg::SignInSignedInTo),
                    self.host
                )
            }
            Stage::Ended(end) => format!("{name} - {}", end.message(language)),
            Stage::KeychainUnreadable => {
                format!("{name} - {}", tr(language, Msg::SignInKeychainUnreadable))
            }
        }
    }
}

fn row_text(row: SignInRow, language: Language) -> &'static str {
    tr(
        language,
        match row {
            SignInRow::Open => Msg::SignInRowOpen,
            SignInRow::Copy => Msg::SignInRowCopy,
            SignInRow::Cancel => Msg::SignInRowCancel,
            SignInRow::TryAgain => Msg::SignInRowTryAgain,
            SignInRow::NewCode => Msg::SignInRowNewCode,
            SignInRow::NotNow => Msg::SignInRowNotNow,
            SignInRow::Close => Msg::SignInRowClose,
        },
    )
}

fn note_text(note: Note, has_address: bool, language: Language) -> &'static str {
    tr(
        language,
        match note {
            Note::BrowserFailed if has_address => Msg::SignInBrowserFailed,
            Note::BrowserFailed => Msg::SignInBrowserFailedNoAddress,
            Note::Copied => Msg::SignInCopied,
            Note::CopyFailed => Msg::SignInCopyFailed,
        },
    )
}

/// One or more lines, broken at spaces to fit the window.
fn info(lines: &mut Vec<SignInLine>, text: &str) {
    for line in wrap_words(text, COLUMNS) {
        lines.push(SignInLine::new(SignInLineKind::Info, line));
    }
}

/// The address as drawn, when there is one and it fits.
fn drawn_address(code: &IssuedCode) -> Option<Vec<String>> {
    let wrapped = wrap_address(code.address.as_deref()?, COLUMNS);
    (wrapped.len() <= MAX_ADDRESS_LINES).then_some(wrapped)
}

/// The address, wrapped rather than cut off, because the host is the part the
/// user needs to check. When the address cannot be drawn, whatever the server
/// sent, the configured host is drawn in its place, so a code is never on
/// screen without it.
fn address_lines(lines: &mut Vec<SignInLine>, code: &IssuedCode, host: &str, language: Language) {
    if let Some(wrapped) = drawn_address(code) {
        for line in wrapped {
            lines.push(SignInLine::new(SignInLineKind::Info, line));
        }
        return;
    }
    info(lines, host);
    // Only point at the Open row when there is one.
    let hint = if code.address.is_some() && code.open_url.is_some() {
        Msg::SignInAddressTooLong
    } else {
        Msg::SignInOpenAccountPage
    };
    info(lines, tr(language, hint));
}

/// Break at spaces to fit. A word wider than the window, such as a long host, is
/// split, because drawing would otherwise cut its end off with no sign of it.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for word in text.split(' ') {
        if used > 0 && used + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        if used > 0 {
            current.push(' ');
            used += 1;
        }
        for ch in word.chars() {
            if used == width {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(ch);
            used += 1;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Break after a `/` where one is in reach, or at the column limit. The text is
/// printable ASCII, so a byte is a column.
fn wrap_address(address: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = address;
    while rest.len() > width {
        let window = &rest[..width];
        let cut = window
            .rfind('/')
            .map(|at| at + 1)
            .filter(|at| *at > width / 2)
            .unwrap_or(width);
        lines.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        lines.push(rest.to_string());
    }
    lines
}

/// `m:ss`, rounded up, so it reads 0:01 until the code has really expired.
fn countdown(left: Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::{
        countdown, opens_at_launch, read_session, wrap_address, wrap_words, LaunchWindow,
        SessionRead, SignInAction, SignInEnd, SignInLineKind, SignInModel, SignInRow, Tick,
        BASE_LINES, COLUMNS, INPUT_GUARD,
    };
    use crate::api::OrbitError;
    use crate::connect::IssuedCode;
    use crate::copy::find_disallowed_copy;
    use crate::i18n::Language;
    use crate::settings::Key;
    use crate::token::{MemoryTokenStore, StoredToken, TokenError, TokenStore};
    use chrono::{DateTime, TimeDelta, Utc};
    use std::time::{Duration, Instant};

    const ENGLISH: Language = Language::English;

    fn code() -> IssuedCode {
        IssuedCode {
            user_code: "ABCD-EFGH".to_string(),
            address: Some("helloorbit.app/rust-preview/pqc/account".to_string()),
            open_url: Some(
                "https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH".to_string(),
            ),
        }
    }

    fn waiting(t0: Instant) -> SignInModel {
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        model.on_code(code(), Duration::from_secs(582), t0);
        model
    }

    fn line_text(model: &SignInModel, now: Instant) -> Vec<String> {
        model
            .lines(now, ENGLISH)
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    fn every_state(t0: Instant) -> Vec<SignInModel> {
        let mut states = vec![
            SignInModel::requesting("helloorbit.app", t0),
            SignInModel::keychain_unreadable("helloorbit.app", t0),
            waiting(t0),
        ];
        let mut reconnecting = waiting(t0);
        assert!(reconnecting.on_connection("ABCD-EFGH", false));
        states.push(reconnecting);
        let mut note = waiting(t0);
        note.note_browser_failed();
        states.push(note);
        let mut copied = waiting(t0);
        copied.note_copied(true);
        states.push(copied);
        let mut copy_failed = waiting(t0);
        copy_failed.note_copied(false);
        states.push(copy_failed);
        let mut no_page = SignInModel::requesting("127.0.0.1:8080", t0);
        no_page.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: None,
                open_url: None,
            },
            Duration::from_secs(60),
            t0,
        );
        states.push(no_page);
        let mut long = SignInModel::requesting("helloorbit.app", t0);
        long.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: Some(format!("helloorbit.app/{}", "a/".repeat(60))),
                open_url: Some("https://helloorbit.app/".to_string()),
            },
            Duration::from_secs(60),
            t0,
        );
        states.push(long);
        let mut three_lines = SignInModel::requesting("helloorbit.app", t0);
        three_lines.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: Some(format!("helloorbit.app/{}", "b".repeat(COLUMNS * 2))),
                open_url: Some("https://helloorbit.app/".to_string()),
            },
            Duration::from_secs(60),
            t0,
        );
        states.push(three_lines);
        // The longest name DNS allows, with no address to draw in its place.
        let longest_host = format!("{}.example", "h".repeat(245));
        let mut long_host = SignInModel::requesting(longest_host.as_str(), t0);
        long_host.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: None,
                open_url: None,
            },
            Duration::from_secs(60),
            t0,
        );
        states.push(long_host);
        let mut long_host_signed_in = SignInModel::requesting(longest_host.as_str(), t0);
        long_host_signed_in.on_signed_in(t0);
        states.push(long_host_signed_in);
        let mut signed_in = waiting(t0);
        signed_in.on_signed_in(t0);
        states.push(signed_in);
        for end in [
            SignInEnd::Denied,
            SignInEnd::Expired,
            SignInEnd::Offline,
            SignInEnd::Unavailable,
            SignInEnd::KeychainFailed,
        ] {
            let mut ended = waiting(t0);
            ended.on_end(end, t0);
            states.push(ended);
        }
        states
    }

    #[test]
    fn the_launch_rule_truth_table() {
        let usable = SessionRead::Usable { until: None };
        let missing = SessionRead::Missing;
        let unreadable = SessionRead::Unreadable;
        for session in [usable, missing, unreadable] {
            assert_eq!(
                opens_at_launch(false, false, session),
                LaunchWindow::None,
                "flag off opens nothing"
            );
            assert_eq!(
                opens_at_launch(true, true, session),
                LaunchWindow::None,
                "--demo opens nothing"
            );
            assert_eq!(opens_at_launch(false, true, session), LaunchWindow::None);
        }
        assert_eq!(opens_at_launch(true, false, usable), LaunchWindow::None);
        assert_eq!(opens_at_launch(true, false, missing), LaunchWindow::SignIn);
        assert_eq!(
            opens_at_launch(true, false, unreadable),
            LaunchWindow::KeychainUnreadable
        );
    }

    struct Broken(TokenError);

    impl TokenStore for Broken {
        fn store(&self, _: &StoredToken) -> Result<(), TokenError> {
            Err(self.0.clone())
        }
        fn load(&self, _: DateTime<Utc>) -> Result<Option<StoredToken>, TokenError> {
            Err(self.0.clone())
        }
        fn clear(&self) -> Result<(), TokenError> {
            Err(self.0.clone())
        }
    }

    #[test]
    fn the_session_read_tells_a_locked_keychain_from_no_record() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        assert_eq!(read_session(&store, now), SessionRead::Missing);
        let expires = now + TimeDelta::seconds(120);
        store
            .store(&StoredToken::issue("access", None, expires, now).unwrap())
            .unwrap();
        assert_eq!(
            read_session(&store, now),
            SessionRead::Usable {
                until: Some(expires)
            }
        );
        store
            .store(&StoredToken::issue("access", Some("refresh".into()), expires, now).unwrap())
            .unwrap();
        assert_eq!(
            read_session(&store, now + TimeDelta::seconds(600)),
            SessionRead::Usable { until: None },
            "an expired access token that can be refreshed is still a session"
        );
        assert_eq!(
            read_session(&Broken(TokenError::Keychain), now),
            SessionRead::Unreadable
        );
        assert_eq!(
            read_session(&Broken(TokenError::Corrupt), now),
            SessionRead::Missing,
            "a new sign-in overwrites a corrupt record"
        );
    }

    #[test]
    fn every_line_of_every_state_fits_and_reads_plainly() {
        let t0 = Instant::now();
        for model in every_state(t0) {
            let lines = model.lines(t0, ENGLISH);
            assert!(lines.len() <= model.max_lines(), "{model:?}");
            for line in &lines {
                assert!(line.text.len() <= COLUMNS, "{}", line.text);
                assert!(line.text.is_ascii(), "{}", line.text);
                assert_eq!(find_disallowed_copy(&line.text), None, "{}", line.text);
            }
            assert_eq!(find_disallowed_copy(&model.title(ENGLISH)), None);
            let focused: Vec<_> = lines.iter().filter(|line| line.focused).collect();
            assert_eq!(focused.len(), usize::from(model.focused().is_some()));
            assert!(focused.iter().all(|line| line.text.starts_with("> ")));
            assert!(lines
                .iter()
                .filter(|line| matches!(line.kind, SignInLineKind::Row(_)) && !line.focused)
                .all(|line| line.text.starts_with("  ")));
        }
    }

    #[test]
    fn the_waiting_state_shows_the_code_the_host_the_warning_and_the_countdown() {
        let t0 = Instant::now();
        let model = waiting(t0);
        let lines = model.lines(t0, ENGLISH);
        assert!(lines
            .iter()
            .any(|line| line.kind == SignInLineKind::Code && line.text == "ABCD-EFGH"));
        let code_at = lines
            .iter()
            .position(|line| line.kind == SignInLineKind::Code)
            .unwrap();
        assert_eq!(
            lines[code_at + 1].kind,
            SignInLineKind::CodeGap,
            "the large code takes two lines, so clicks still map to rows"
        );
        let text = line_text(&model, t0);
        assert!(text.contains(&"helloorbit.app/rust-preview/pqc/account".to_string()));
        assert!(text.contains(&"Only enter this code if you started this sign-in.".to_string()));
        assert!(text.contains(&"Code expires in 9:42".to_string()));
        assert!(text.contains(&"Waiting for approval...".to_string()));
        assert!(
            text.iter().all(|line| !line.contains("user_code=")),
            "the complete address is never drawn"
        );
        assert_eq!(
            model.rows(),
            [SignInRow::Open, SignInRow::Copy, SignInRow::Cancel]
        );
        assert_eq!(model.focused(), Some(SignInRow::Open));
    }

    #[test]
    fn the_title_carries_the_state_and_the_code_but_not_the_countdown() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        assert_eq!(
            model.title(ENGLISH),
            "Stardust sign-in - Enter code ABCD-EFGH"
        );
        let before = model.title(ENGLISH);
        assert_eq!(model.tick(t0 + Duration::from_secs(1)), Tick::Changed);
        assert_eq!(model.title(ENGLISH), before);
        model.on_connection("ABCD-EFGH", false);
        assert_eq!(
            model.title(ENGLISH),
            "Stardust sign-in - Enter code ABCD-EFGH - Cannot reach ORBIT. Still trying"
        );
        model.on_end(SignInEnd::Denied, t0);
        assert_eq!(
            model.title(ENGLISH),
            "Stardust sign-in - ORBIT denied this sign-in"
        );
    }

    #[test]
    fn each_end_has_its_own_message_and_rows() {
        let t0 = Instant::now();
        let cases = [
            (
                SignInEnd::Denied,
                "ORBIT denied this sign-in",
                SignInRow::TryAgain,
            ),
            (SignInEnd::Expired, "This code expired", SignInRow::NewCode),
            (
                SignInEnd::Offline,
                "Cannot reach ORBIT",
                SignInRow::TryAgain,
            ),
            (
                SignInEnd::Unavailable,
                "Sign-in is not available on this server",
                SignInRow::TryAgain,
            ),
            (
                SignInEnd::KeychainFailed,
                "ORBIT approved, but the system keychain did not save",
                SignInRow::TryAgain,
            ),
        ];
        for (end, message, first) in cases {
            let mut model = waiting(t0);
            model.on_end(end, t0);
            assert!(
                line_text(&model, t0).contains(&message.to_string()),
                "{end:?}"
            );
            assert_eq!(model.rows(), [first, SignInRow::NotNow], "{end:?}");
            assert_eq!(model.focused(), Some(first), "focus never lands on Cancel");
            let after = t0 + INPUT_GUARD;
            assert_eq!(
                model.handle_key(Key::Activate, after),
                SignInAction::Restart
            );
            assert_eq!(model.handle_key(Key::Escape, after), SignInAction::Close);
        }
    }

    #[test]
    fn errors_map_to_ends() {
        assert_eq!(
            SignInEnd::from_error(&OrbitError::Denied),
            SignInEnd::Denied
        );
        assert_eq!(
            SignInEnd::from_error(&OrbitError::Expired),
            SignInEnd::Expired
        );
        assert_eq!(
            SignInEnd::from_error(&OrbitError::Unreachable),
            SignInEnd::Offline
        );
        for err in [
            OrbitError::FeatureOff,
            OrbitError::Unauthorized,
            OrbitError::Response,
            OrbitError::Refused,
            OrbitError::ServerBase,
        ] {
            assert_eq!(SignInEnd::from_error(&err), SignInEnd::Unavailable, "{err}");
        }
    }

    #[test]
    fn the_keyboard_reaches_every_row() {
        let t0 = Instant::now();
        let after = t0 + INPUT_GUARD;
        let mut model = waiting(t0);
        assert_eq!(
            model.handle_key(Key::Activate, after),
            SignInAction::Open(
                "https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH".to_string()
            )
        );
        assert_eq!(model.handle_key(Key::Tab, after), SignInAction::Moved);
        assert_eq!(
            model.handle_key(Key::Activate, after),
            SignInAction::Copy("ABCD-EFGH".to_string())
        );
        assert_eq!(model.handle_key(Key::Down, after), SignInAction::Moved);
        assert_eq!(model.focused(), Some(SignInRow::Cancel));
        assert_eq!(model.handle_key(Key::Activate, after), SignInAction::Close);
        assert_eq!(model.handle_key(Key::Down, after), SignInAction::Moved);
        assert_eq!(model.focused(), Some(SignInRow::Open), "wraps around");
        assert_eq!(model.handle_key(Key::BackTab, after), SignInAction::Moved);
        assert_eq!(model.focused(), Some(SignInRow::Cancel));
        model.handle_key(Key::Home, after);
        assert_eq!(model.focused(), Some(SignInRow::Open));
        model.handle_key(Key::End, after);
        assert_eq!(model.focused(), Some(SignInRow::Cancel));
        assert_eq!(model.handle_key(Key::Left, after), SignInAction::None);
        assert_eq!(model.handle_key(Key::Right, after), SignInAction::None);
        assert_eq!(model.handle_key(Key::Escape, after), SignInAction::Close);
    }

    #[test]
    fn enter_cannot_end_a_request_that_is_still_out_but_esc_and_a_click_can() {
        let t0 = Instant::now();
        let after = t0 + INPUT_GUARD;
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        assert_eq!(model.focused(), None);
        assert_eq!(model.rows(), [SignInRow::Cancel]);
        for key in [
            Key::Activate,
            Key::Up,
            Key::Down,
            Key::Tab,
            Key::BackTab,
            Key::Home,
            Key::End,
            Key::Left,
            Key::Right,
        ] {
            assert_eq!(model.handle_key(key, after), SignInAction::None, "{key:?}");
        }
        assert_eq!(model.focused(), None);
        let lines = model.lines(after, ENGLISH);
        assert!(lines.iter().all(|line| !line.focused));
        assert!(
            lines.iter().any(|line| line.text == "Esc cancels"),
            "the hint names the key that works"
        );
        let cancel_line = lines
            .iter()
            .position(|line| line.kind == SignInLineKind::Row(SignInRow::Cancel))
            .unwrap();
        assert_eq!(model.click(cancel_line, after), SignInAction::Close);
        assert_eq!(model.handle_key(Key::Escape, after), SignInAction::Close);
    }

    #[test]
    fn a_late_code_or_ending_does_not_undo_signed_in() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        model.on_signed_in(t0);
        let closes = model.next_wake(t0);
        model.on_code(code(), Duration::from_secs(600), t0);
        model.on_end(SignInEnd::Offline, t0);
        assert!(line_text(&model, t0).contains(&"Signed in to helloorbit.app".to_string()));
        assert_eq!(model.rows(), [SignInRow::Close]);
        assert_eq!(model.next_wake(t0), closes, "it still closes on time");
    }

    #[test]
    fn the_host_stays_on_screen_whatever_the_server_sent_for_the_address() {
        let t0 = Instant::now();
        let open = Some("https://helloorbit.app/".to_string());
        let too_long = Some(format!("helloorbit.app/{}", "a".repeat(COLUMNS * 4)));
        for (address, open_url, message) in [
            (
                too_long.clone(),
                open.clone(),
                "The address is too long to show. Use the Open row.",
            ),
            (
                None,
                open.clone(),
                "Open the ORBIT account page in your browser.",
            ),
            (
                too_long.clone(),
                None,
                "Open the ORBIT account page in your browser.",
            ),
            (None, None, "Open the ORBIT account page in your browser."),
        ] {
            let mut model = SignInModel::requesting("helloorbit.app", t0);
            model.on_code(
                IssuedCode {
                    user_code: "ABCD-EFGH".to_string(),
                    address,
                    open_url: open_url.clone(),
                },
                Duration::from_secs(600),
                t0,
            );
            let text = line_text(&model, t0);
            assert!(text.contains(&"helloorbit.app".to_string()), "{text:?}");
            assert!(text.contains(&message.to_string()), "{text:?}");
            assert_eq!(
                model.rows().contains(&SignInRow::Open),
                open_url.is_some(),
                "the window points at an Open row only when there is one"
            );
        }
    }

    #[test]
    fn the_browser_note_points_above_only_when_an_address_is_there() {
        let t0 = Instant::now();
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        model.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: Some(format!("helloorbit.app/{}", "a".repeat(COLUMNS * 4))),
                open_url: Some("https://helloorbit.app/".to_string()),
            },
            Duration::from_secs(600),
            t0,
        );
        model.note_browser_failed();
        let text = line_text(&model, t0);
        assert!(
            text.contains(&"No browser opened. Go to the ORBIT account page".to_string()),
            "{text:?}"
        );
        assert!(!text.iter().any(|line| line.contains("address above")));
    }

    #[test]
    fn a_long_host_is_wrapped_and_the_window_grows_to_hold_it() {
        let t0 = Instant::now();
        assert_eq!(SignInModel::requesting("h", t0).max_lines(), BASE_LINES);
        let host = format!("{}.example", "h".repeat(120));
        let mut model = SignInModel::requesting(host.as_str(), t0);
        assert_eq!(model.max_lines(), BASE_LINES + 1);
        model.on_signed_in(t0);
        let lines = line_text(&model, t0);
        assert!(lines.iter().all(|line| line.chars().count() <= COLUMNS));
        assert!(
            lines.join("").contains(&host),
            "nothing is cut off: {lines:?}"
        );
        // A host that is not ASCII must not split inside a letter.
        let wrapped = wrap_words(&"\u{e9}".repeat(COLUMNS + 3), COLUMNS);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].chars().count(), COLUMNS);
    }

    #[test]
    fn a_stray_key_or_click_right_after_a_change_does_nothing() {
        let t0 = Instant::now();
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        let during = t0 + INPUT_GUARD - Duration::from_millis(1);
        assert_eq!(model.handle_key(Key::Activate, during), SignInAction::None);
        assert_eq!(model.handle_key(Key::Escape, during), SignInAction::None);
        let cancel_line = model
            .lines(t0, ENGLISH)
            .iter()
            .position(|line| line.kind == SignInLineKind::Row(SignInRow::Cancel))
            .unwrap();
        assert_eq!(model.click(cancel_line, during), SignInAction::None);

        // The code lands: focus goes to the first row, and the guard restarts,
        // so an Enter meant for the last state does not open a browser.
        let landed = t0 + Duration::from_secs(1);
        model.on_code(code(), Duration::from_secs(600), landed);
        assert_eq!(model.focused(), Some(SignInRow::Open));
        assert_eq!(model.handle_key(Key::Activate, landed), SignInAction::None);
        assert!(matches!(
            model.handle_key(Key::Activate, landed + INPUT_GUARD),
            SignInAction::Open(_)
        ));

        // The second half of a double click on Try again must not hit Cancel.
        let mut ended = waiting(t0);
        ended.on_end(SignInEnd::Offline, t0);
        let retried = t0 + Duration::from_secs(2);
        ended.start_over(retried);
        let cancel_line = ended
            .lines(retried, ENGLISH)
            .iter()
            .position(|line| line.kind == SignInLineKind::Row(SignInRow::Cancel))
            .unwrap();
        assert_eq!(ended.click(cancel_line, retried), SignInAction::None);
    }

    #[test]
    fn a_click_focuses_and_chooses_only_row_lines() {
        let t0 = Instant::now();
        let after = t0 + INPUT_GUARD;
        let mut model = waiting(t0);
        let lines = model.lines(after, ENGLISH);
        let cancel = lines
            .iter()
            .position(|line| line.kind == SignInLineKind::Row(SignInRow::Cancel))
            .unwrap();
        assert_eq!(model.click(cancel, after), SignInAction::Close);
        assert_eq!(model.focused(), Some(SignInRow::Cancel));
        assert_eq!(model.click(0, after), SignInAction::None);
        let code_line = lines
            .iter()
            .position(|line| line.kind == SignInLineKind::Code)
            .unwrap();
        assert_eq!(model.click(code_line, after), SignInAction::None);
        assert_eq!(model.click(code_line + 1, after), SignInAction::None);
        assert_eq!(model.click(99, after), SignInAction::None);
    }

    #[test]
    fn a_note_does_not_move_the_rows_under_the_pointer() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        let before = model.lines(t0, ENGLISH);
        model.note_copied(true);
        let after = model.lines(t0, ENGLISH);
        assert_eq!(before.len(), after.len());
        let rows = |lines: &[super::SignInLine]| {
            lines
                .iter()
                .position(|line| matches!(line.kind, SignInLineKind::Row(_)))
        };
        assert_eq!(rows(&before), rows(&after));
        assert!(line_text(&model, t0).contains(&"Code copied".to_string()));
        model.note_copied(false);
        assert!(
            line_text(&model, t0).contains(&"Could not copy. Type the code instead".to_string())
        );
        model.note_browser_failed();
        assert!(line_text(&model, t0)
            .contains(&"Could not open a browser. Type the address above".to_string()));
    }

    #[test]
    fn with_no_allowed_page_the_open_row_is_left_out_and_the_window_says_where_to_go() {
        let t0 = Instant::now();
        let mut model = SignInModel::requesting("127.0.0.1:8080", t0);
        model.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: None,
                open_url: None,
            },
            Duration::from_secs(600),
            t0,
        );
        assert_eq!(model.rows(), [SignInRow::Copy, SignInRow::Cancel]);
        assert!(line_text(&model, t0)
            .contains(&"Open the ORBIT account page in your browser.".to_string()));
    }

    #[test]
    fn a_long_address_wraps_and_one_too_long_is_replaced_by_a_plain_note() {
        let t0 = Instant::now();
        let long = format!("helloorbit.app/{}/account", "path".repeat(14));
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        model.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: Some(long.clone()),
                open_url: Some("https://helloorbit.app/".to_string()),
            },
            Duration::from_secs(600),
            t0,
        );
        let text = line_text(&model, t0).join("");
        assert!(text.contains(&long), "nothing is cut off");
        let too_long = format!("helloorbit.app/{}", "a".repeat(COLUMNS * 4));
        model.on_code(
            IssuedCode {
                user_code: "ABCD-EFGH".to_string(),
                address: Some(too_long),
                open_url: Some("https://helloorbit.app/".to_string()),
            },
            Duration::from_secs(600),
            t0,
        );
        assert!(line_text(&model, t0)
            .contains(&"The address is too long to show. Use the Open row.".to_string()));
    }

    #[test]
    fn wrapping_keeps_every_character() {
        for address in [
            "helloorbit.app/rust-preview/pqc/account".to_string(),
            format!("h.example/{}", "seg/".repeat(40)),
            "x".repeat(130),
        ] {
            let wrapped = wrap_address(&address, COLUMNS);
            assert_eq!(wrapped.join(""), address);
            assert!(wrapped.iter().all(|line| line.len() <= COLUMNS));
        }
        let text = "ORBIT approved, but the system keychain did not save the sign-in";
        let wrapped = wrap_words(text, COLUMNS);
        assert_eq!(wrapped.join(" "), text);
        assert!(wrapped.iter().all(|line| line.len() <= COLUMNS));
    }

    #[test]
    fn the_countdown_reads_minutes_and_seconds_and_rounds_up() {
        assert_eq!(countdown(Duration::from_secs(582)), "9:42");
        assert_eq!(countdown(Duration::from_secs(60)), "1:00");
        assert_eq!(countdown(Duration::from_millis(100)), "0:01");
        assert_eq!(countdown(Duration::ZERO), "0:00");
    }

    #[test]
    fn the_window_expires_by_itself_and_wakes_only_while_it_has_a_clock() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        assert!(model.next_wake(t0).is_some());
        assert_eq!(model.tick(t0 + Duration::from_secs(5)), Tick::Changed);
        let end = t0 + Duration::from_secs(582);
        assert_eq!(model.tick(end), Tick::Changed);
        assert!(line_text(&model, end).contains(&"This code expired".to_string()));
        assert_eq!(model.next_wake(end), None, "nothing left to count down");
        assert_eq!(model.tick(end + Duration::from_secs(1)), Tick::Idle);
        assert_eq!(model.focused(), Some(SignInRow::NewCode));
        // The check timer's own report of the same end changes nothing.
        model.handle_key(Key::Down, end + INPUT_GUARD);
        model.on_end(SignInEnd::Expired, end + INPUT_GUARD);
        assert_eq!(model.focused(), Some(SignInRow::NotNow));
        for model in [
            SignInModel::requesting("h", t0),
            SignInModel::keychain_unreadable("h", t0),
        ] {
            assert_eq!(model.next_wake(t0), None);
        }
    }

    #[test]
    fn the_next_wake_lands_on_the_next_whole_second_of_the_countdown() {
        let t0 = Instant::now();
        let mut model = SignInModel::requesting("h", t0);
        model.on_code(code(), Duration::from_millis(2_500), t0);
        assert_eq!(model.next_wake(t0), Some(t0 + Duration::from_millis(500)));
        let one = t0 + Duration::from_millis(500);
        assert_eq!(model.next_wake(one), Some(one + Duration::from_secs(1)));
        let last = t0 + Duration::from_millis(2_500);
        assert_eq!(model.next_wake(last), Some(last), "wakes to expire at once");
    }

    #[test]
    fn signed_in_says_where_and_closes_by_itself() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        model.on_signed_in(t0);
        assert!(line_text(&model, t0).contains(&"Signed in to helloorbit.app".to_string()));
        assert_eq!(model.rows(), [SignInRow::Close]);
        assert_eq!(
            model.next_wake(t0),
            Some(t0 + Duration::from_secs(2)),
            "closes in two seconds"
        );
        assert_eq!(model.tick(t0 + Duration::from_secs(1)), Tick::Idle);
        assert_eq!(model.tick(t0 + Duration::from_secs(2)), Tick::Close);
        assert_eq!(
            model.handle_key(Key::Activate, t0 + INPUT_GUARD),
            SignInAction::Close
        );
    }

    #[test]
    fn reconnecting_only_applies_to_the_code_on_screen() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        assert!(!model.on_connection("OTHR-CODE", false), "an old flow");
        assert!(!model.on_connection("ABCD-EFGH", true), "no change");
        assert!(model.on_connection("ABCD-EFGH", false));
        assert!(line_text(&model, t0).contains(&"Cannot reach ORBIT. Still trying".to_string()));
        assert!(
            line_text(&model, t0).contains(&"Code expires in 9:42".to_string()),
            "the code and the countdown stay"
        );
        assert_eq!(model.rows()[0], SignInRow::Open, "the rows stay");
        assert!(model.on_connection("ABCD-EFGH", true));
        assert!(line_text(&model, t0).contains(&"Waiting for approval...".to_string()));
        let mut requesting = SignInModel::requesting("h", t0);
        assert!(!requesting.on_connection("ABCD-EFGH", false));
    }

    #[test]
    fn starting_over_goes_back_to_asking_for_a_code() {
        let t0 = Instant::now();
        let mut model = waiting(t0);
        model.on_end(SignInEnd::Denied, t0);
        assert_eq!(model.rows(), [SignInRow::TryAgain, SignInRow::NotNow]);
        model.start_over(t0);
        assert_eq!(model.rows(), [SignInRow::Cancel]);
        assert!(line_text(&model, t0).contains(&"Asking ORBIT for a code...".to_string()));
        let keychain = SignInModel::keychain_unreadable("h", t0);
        assert!(keychain.is_keychain_unreadable());
        assert_eq!(keychain.rows(), [SignInRow::TryAgain, SignInRow::NotNow]);
    }

    #[test]
    fn signed_in_shows_the_host_only_after_approval_and_nothing_secret_is_held() {
        let t0 = Instant::now();
        let model = waiting(t0);
        let debug = format!("{model:?}");
        assert!(debug.contains("ABCD-EFGH"));
        assert!(!debug.to_ascii_lowercase().contains("device_code"));
        assert!(!debug.to_ascii_lowercase().contains("token"));
    }
}
