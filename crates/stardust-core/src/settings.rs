//! Settings and first-run window model.
//!
//! The window is drawn by the app. This module owns the rows, keyboard
//! focus, and what each key or click does, so the behaviour is testable
//! without a display. Every row is reachable with the keyboard alone:
//! Up/Down or Tab/Shift+Tab move, Enter or Space changes, Left/Right step a
//! choice, Esc closes. The focused row is marked with `>` as well as by
//! colour, so focus does not depend on colour alone.
//!
//! With the integrations panel on, one more row opens a paged page of
//! integrations in the same window, and turning one on for the first time
//! shows what it sends and asks. Neither page shows a URL or a secret: the
//! view holds host names only.

use crate::consent::{destination_host, has_valid_consent, what_is_sent, wrap};
use crate::delivery::DeliveryBoard;
use crate::i18n::{interval_label, tr, Language, Msg};
use crate::integrations::{IntegrationKind, IntegrationRecord};
use crate::pet::{PetScale, PetSkin};
use crate::schedule::QUICK_PICK_MINUTES;

/// Widest settings line, in characters. The window draws no further.
pub const SETTINGS_COLUMNS: usize = 52;
/// Integrations on one panel page.
pub const PANEL_PAGE_ENTRIES: usize = 3;
/// Lines the window must have room for with the panel on. The window is not
/// resized while open, so this is the tallest page either screen can draw.
pub const PANEL_LINES: usize = 14;
/// A host name longer than this many lines is not shown on the consent page,
/// so the page still fits. It sends the user to the command line instead.
const CONSENT_HOST_LINES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Account,
    CheckEvery,
    StartAtLogin,
    ShowCharacter,
    PetSkin,
    PetSize,
    Integrations,
    Done,
}

pub const ROWS: [Row; 7] = [
    Row::Account,
    Row::CheckEvery,
    Row::StartAtLogin,
    Row::ShowCharacter,
    Row::PetSkin,
    Row::PetSize,
    Row::Done,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInStage {
    Idle,
    /// The device code was issued. `user_code` is the short code to type in
    /// the browser. It is not a secret.
    Waiting {
        user_code: String,
    },
    Failed,
}

/// Current values shown in the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsView {
    pub signed_in: bool,
    pub sign_in: SignInStage,
    pub interval_minutes: Option<u32>,
    pub start_at_login: bool,
    pub start_at_login_available: bool,
    pub character_visible: bool,
    pub skin: PetSkin,
    pub scale: PetScale,
    /// The ORBIT host shown on the account row, without a scheme.
    pub server_host: String,
    /// Only with the integrations panel on.
    pub panel: Option<PanelView>,
}

/// One integration as the panel shows it. Holds a host name, never a URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelEntry {
    pub id: String,
    pub kind: IntegrationKind,
    /// Alerts can go out: turned on, and accepted where that is needed.
    pub on: bool,
    pub host: Option<String>,
    /// The last delivery result, or why there is none.
    pub status: String,
    pub needs_consent: bool,
    /// Turned on in the config, but sending nothing until the user accepts.
    pub waiting_for_consent: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelView {
    pub entries: Vec<PanelEntry>,
}

/// The panel's view of the saved integrations and their last results.
pub fn panel_view(
    records: &[IntegrationRecord],
    board: &DeliveryBoard,
    language: Language,
) -> PanelView {
    let entries = records
        .iter()
        .map(|record| {
            let delivers = record.kind.delivers_alerts();
            let needs_consent = delivers && !has_valid_consent(record);
            let host = if delivers {
                destination_host(record)
            } else {
                record.host.clone()
            };
            let status = if !delivers {
                tr(language, Msg::StatusNoAlertDelivery)
            } else if host.is_none() {
                tr(language, Msg::StatusNoDestination)
            } else if record.enabled && needs_consent {
                tr(language, Msg::OutcomeNeedsConsent)
            } else {
                board.get(&record.id).map_or_else(
                    || tr(language, Msg::StatusNoDeliveryYet),
                    |outcome| outcome.label(language),
                )
            };
            PanelEntry {
                id: record.id.clone(),
                kind: record.kind,
                on: record.enabled && !needs_consent,
                waiting_for_consent: record.enabled && needs_consent && host.is_some(),
                host,
                status: status.to_string(),
                needs_consent,
            }
        })
        .collect();
    PanelView { entries }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Activate,
    Escape,
    Home,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    None,
    /// Focus moved; redraw only.
    Moved,
    Close,
    StartSignIn,
    SignOut,
    SetInterval(u32),
    SetStartAtLogin(bool),
    SetCharacterVisible(bool),
    SetSkin(PetSkin),
    SetScale(PetScale),
    /// Turn an integration on or off. Its consent is already settled.
    SetIntegration {
        id: String,
        on: bool,
    },
    /// The user accepted what this integration sends, to `host`, the host the
    /// page showed. Record it, only for that host, and turn the integration
    /// on.
    AcceptConsent {
        id: String,
        host: String,
    },
    SendTest {
        id: String,
    },
}

/// A row on the integrations or consent page. The numbers index
/// [`PanelView::entries`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelRow {
    Toggle(usize),
    Test(usize),
    NextPage,
    Back,
    Accept,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Title,
    Info,
    Row(Row),
    Panel(PanelRow),
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
    pub focused: bool,
}

/// Which page the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Main,
    Panel {
        page: usize,
        focus: usize,
    },
    /// `page` and `focus` are where Cancel returns to on the panel.
    Consent {
        entry: usize,
        page: usize,
        focus: usize,
        on_accept: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsModel {
    focus: Row,
    first_run: bool,
    screen: Screen,
}

/// Rows shown for this view. The start-at-login row is left out where the
/// platform has no entry for it, and the integrations row is there only with
/// the panel on.
pub fn visible_rows(view: &SettingsView) -> Vec<Row> {
    let mut rows: Vec<Row> = ROWS
        .into_iter()
        .filter(|row| *row != Row::StartAtLogin || view.start_at_login_available)
        .collect();
    if view.panel.is_some() {
        rows.insert(rows.len() - 1, Row::Integrations);
    }
    rows
}

impl SettingsModel {
    pub fn new(first_run: bool) -> Self {
        Self {
            focus: Row::Account,
            first_run,
            screen: Screen::Main,
        }
    }

    pub fn focused(&self) -> Row {
        self.focus
    }

    /// Show the integrations page, from its row or the tray.
    pub fn open_panel(&mut self) {
        self.screen = Screen::Panel { page: 0, focus: 0 };
    }

    /// The integrations or consent page is showing.
    pub fn in_panel(&self) -> bool {
        self.screen != Screen::Main
    }

    /// Which page is showing, as a value that changes when the page does.
    /// The window uses it to drop a click that lands just after the page
    /// under the pointer changed.
    pub fn page_marker(&self) -> (u8, usize) {
        match self.screen {
            Screen::Main => (0, 0),
            Screen::Panel { page, .. } => (1, page),
            Screen::Consent { entry, .. } => (2, entry),
        }
    }

    fn panel_key(
        &mut self,
        key: Key,
        panel: &PanelView,
        page: usize,
        focus: usize,
    ) -> SettingsAction {
        let page = page.min(page_count(panel) - 1);
        let rows = page_rows(panel, page);
        let last = rows.len() - 1;
        let focus = focus.min(last);
        let moved_to = match key {
            Key::Up | Key::BackTab => {
                if focus == 0 {
                    last
                } else {
                    focus - 1
                }
            }
            Key::Down | Key::Tab => {
                if focus == last {
                    0
                } else {
                    focus + 1
                }
            }
            Key::Home => 0,
            Key::End => last,
            Key::Escape => {
                self.screen = Screen::Main;
                return SettingsAction::Moved;
            }
            Key::Left | Key::Right => {
                let pages = page_count(panel);
                if pages == 1 {
                    return SettingsAction::None;
                }
                let next = if key == Key::Right {
                    (page + 1) % pages
                } else {
                    (page + pages - 1) % pages
                };
                self.screen = Screen::Panel {
                    page: next,
                    focus: 0,
                };
                return SettingsAction::Moved;
            }
            Key::Activate => return self.panel_activate(rows[focus], panel, page, focus),
        };
        self.screen = Screen::Panel {
            page,
            focus: moved_to,
        };
        SettingsAction::Moved
    }

    fn panel_activate(
        &mut self,
        row: PanelRow,
        panel: &PanelView,
        page: usize,
        focus: usize,
    ) -> SettingsAction {
        match row {
            PanelRow::Toggle(index) | PanelRow::Test(index) => {
                let Some(entry) = panel.entries.get(index) else {
                    return SettingsAction::None;
                };
                if entry.needs_consent {
                    if entry.host.is_none() {
                        // The status line already says no destination is saved.
                        return SettingsAction::None;
                    }
                    // Enter twice must not accept, so focus starts on Cancel.
                    self.screen = Screen::Consent {
                        entry: index,
                        page,
                        focus,
                        on_accept: false,
                    };
                    return SettingsAction::Moved;
                }
                if matches!(row, PanelRow::Test(_)) {
                    SettingsAction::SendTest {
                        id: entry.id.clone(),
                    }
                } else {
                    SettingsAction::SetIntegration {
                        id: entry.id.clone(),
                        on: !entry.on,
                    }
                }
            }
            PanelRow::NextPage => {
                self.screen = Screen::Panel {
                    page: (page + 1) % page_count(panel),
                    focus: 0,
                };
                SettingsAction::Moved
            }
            PanelRow::Back => {
                self.screen = Screen::Main;
                SettingsAction::Moved
            }
            PanelRow::Accept | PanelRow::Cancel => SettingsAction::None,
        }
    }

    fn consent_key(&mut self, key: Key, panel: &PanelView) -> SettingsAction {
        let Screen::Consent {
            entry,
            page,
            focus,
            on_accept,
        } = self.screen
        else {
            return SettingsAction::None;
        };
        let back = Screen::Panel { page, focus };
        let Some(shown) = panel.entries.get(entry) else {
            self.screen = back;
            return SettingsAction::Moved;
        };
        let can_accept = consent_host_lines(shown).is_some();
        match key {
            Key::Escape => {
                self.screen = back;
                SettingsAction::Moved
            }
            Key::Activate => {
                self.screen = back;
                match (&shown.host, on_accept && can_accept) {
                    (Some(host), true) => SettingsAction::AcceptConsent {
                        id: shown.id.clone(),
                        host: host.clone(),
                    },
                    _ => SettingsAction::Moved,
                }
            }
            Key::Left | Key::Right => SettingsAction::None,
            Key::Up | Key::Down | Key::Tab | Key::BackTab | Key::Home | Key::End => {
                let next = match key {
                    Key::Home => true,
                    Key::End => false,
                    _ => !on_accept,
                };
                self.screen = Screen::Consent {
                    entry,
                    page,
                    focus,
                    on_accept: next && can_accept,
                };
                SettingsAction::Moved
            }
        }
    }

    fn move_focus(&mut self, view: &SettingsView, delta: isize) -> SettingsAction {
        let rows = visible_rows(view);
        let count = rows.len() as isize;
        let at = rows.iter().position(|row| *row == self.focus).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(count);
        self.focus = rows[next as usize];
        SettingsAction::Moved
    }

    pub fn first_run(&self) -> bool {
        self.first_run
    }

    pub fn handle_key(&mut self, key: Key, view: &SettingsView) -> SettingsAction {
        if self.screen != Screen::Main {
            let Some(panel) = view.panel.as_ref() else {
                self.screen = Screen::Main;
                return SettingsAction::Moved;
            };
            return match self.screen {
                Screen::Panel { page, focus } => self.panel_key(key, panel, page, focus),
                _ => self.consent_key(key, panel),
            };
        }
        match key {
            Key::Activate if self.focus == Row::Integrations => {
                self.open_panel();
                SettingsAction::Moved
            }
            Key::Up | Key::BackTab => self.move_focus(view, -1),
            Key::Down | Key::Tab => self.move_focus(view, 1),
            Key::Home => {
                self.focus = Row::Account;
                SettingsAction::Moved
            }
            Key::End => {
                self.focus = Row::Done;
                SettingsAction::Moved
            }
            Key::Escape => SettingsAction::Close,
            Key::Activate => activate(self.focused(), view, true),
            Key::Right => step(self.focused(), view, true),
            Key::Left => step(self.focused(), view, false),
        }
    }

    /// A click on line `line_index` of [`SettingsModel::lines`] focuses that
    /// row and activates it. Other lines do nothing.
    pub fn click(&mut self, line_index: usize, view: &SettingsView) -> SettingsAction {
        let lines = self.lines(view, Language::English);
        match lines.get(line_index).map(|line| line.kind) {
            Some(LineKind::Row(row)) => {
                self.focus = row;
                if row == Row::Integrations {
                    self.open_panel();
                    return SettingsAction::Moved;
                }
                activate(row, view, true)
            }
            Some(LineKind::Panel(row)) => {
                let Some(panel) = view.panel.as_ref() else {
                    return SettingsAction::None;
                };
                match self.screen {
                    Screen::Panel { page, .. } => {
                        let Some(focus) = page_rows(panel, page).iter().position(|at| *at == row)
                        else {
                            return SettingsAction::None;
                        };
                        self.screen = Screen::Panel { page, focus };
                    }
                    Screen::Consent {
                        entry,
                        page,
                        focus,
                        on_accept,
                    } => {
                        if row == PanelRow::Accept && !on_accept {
                            // A click only focuses Accept; a second one takes
                            // it. The second click of a double-click that
                            // opened this page can land here by chance.
                            self.screen = Screen::Consent {
                                entry,
                                page,
                                focus,
                                on_accept: true,
                            };
                            return SettingsAction::Moved;
                        }
                        self.screen = Screen::Consent {
                            entry,
                            page,
                            focus,
                            on_accept: row == PanelRow::Accept,
                        };
                    }
                    Screen::Main => return SettingsAction::None,
                }
                self.handle_key(Key::Activate, view)
            }
            _ => SettingsAction::None,
        }
    }

    pub fn lines(&self, view: &SettingsView, language: Language) -> Vec<Line> {
        let empty = PanelView::default();
        let panel = view.panel.as_ref().unwrap_or(&empty);
        match self.screen {
            Screen::Main => self.main_lines(view, language),
            Screen::Panel { page, focus } => panel_lines(panel, page, focus, language),
            Screen::Consent {
                entry, on_accept, ..
            } => consent_lines(panel, entry, on_accept, language),
        }
    }

    fn main_lines(&self, view: &SettingsView, language: Language) -> Vec<Line> {
        let title = if self.first_run {
            tr(language, Msg::Welcome)
        } else {
            tr(language, Msg::SettingsTitle)
        };
        let mut lines = vec![Line {
            kind: LineKind::Title,
            text: title.to_string(),
            focused: false,
        }];
        let status = match (&view.sign_in, view.signed_in) {
            (SignInStage::Waiting { user_code }, _) => {
                format!("{}: {user_code}", tr(language, Msg::WaitingForApproval))
            }
            (SignInStage::Failed, _) => tr(language, Msg::SignInFailed).to_string(),
            (SignInStage::Idle, true) => {
                format!("{} - {}", tr(language, Msg::SignedIn), view.server_host)
            }
            (SignInStage::Idle, false) => {
                format!("{} - {}", tr(language, Msg::NotSignedIn), view.server_host)
            }
        };
        lines.push(Line {
            kind: LineKind::Info,
            text: status,
            focused: false,
        });
        for row in visible_rows(view) {
            let focused = row == self.focused();
            let marker = if focused { "> " } else { "  " };
            lines.push(Line {
                kind: LineKind::Row(row),
                text: format!("{marker}{}", row_text(row, view, language)),
                focused,
            });
        }
        lines.push(Line {
            kind: LineKind::Hint,
            text: tr(language, Msg::KeysHint).to_string(),
            focused: false,
        });
        lines
    }

    /// Short description of the focused row for the window title, which the
    /// OS exposes to assistive technology.
    pub fn focus_label(&self, view: &SettingsView, language: Language) -> String {
        if self.screen == Screen::Main {
            return format!(
                "{} - {}",
                tr(language, Msg::SettingsTitle),
                row_text(self.focused(), view, language)
            );
        }
        let lines = self.lines(view, language);
        let settings = tr(language, Msg::SettingsTitle);
        let Some(at) = lines.iter().position(|line| line.focused) else {
            return settings.to_string();
        };
        let focused = lines[at].text.trim_start_matches("> ").trim();
        if matches!(self.screen, Screen::Consent { .. }) {
            // The consent page's wording has to reach a screen reader too.
            let question: Vec<&str> = lines
                .iter()
                .filter(|line| matches!(line.kind, LineKind::Title | LineKind::Info))
                .map(|line| line.text.as_str())
                .collect();
            return format!("{settings} - {} - {focused}", question.join(" "));
        }
        match lines.get(at + 1) {
            Some(next) if next.kind == LineKind::Info => {
                format!("{settings} - {focused} - {}", next.text.trim())
            }
            _ => format!("{settings} - {focused}"),
        }
    }
}

fn page_count(panel: &PanelView) -> usize {
    panel.entries.len().div_ceil(PANEL_PAGE_ENTRIES).max(1)
}

/// The rows on one panel page, in the order focus moves through them. Send
/// test is there only for a sink with a destination.
fn page_rows(panel: &PanelView, page: usize) -> Vec<PanelRow> {
    let start = page.saturating_mul(PANEL_PAGE_ENTRIES);
    let mut rows = Vec::new();
    for (index, entry) in panel
        .entries
        .iter()
        .enumerate()
        .skip(start)
        .take(PANEL_PAGE_ENTRIES)
    {
        rows.push(PanelRow::Toggle(index));
        if entry.kind.delivers_alerts() && entry.host.is_some() {
            rows.push(PanelRow::Test(index));
        }
    }
    if page_count(panel) > 1 {
        rows.push(PanelRow::NextPage);
    }
    rows.push(PanelRow::Back);
    rows
}

/// Keep the end of a host name, which is the part that says whose it is.
fn host_tail(host: &str, max: usize) -> String {
    let count = host.chars().count();
    if count <= max {
        return host.to_string();
    }
    let keep = max.saturating_sub(2);
    let tail: String = host.chars().skip(count - keep).collect();
    format!("..{tail}")
}

fn clip_line(text: String) -> String {
    if text.chars().count() <= SETTINGS_COLUMNS {
        return text;
    }
    let kept: String = text.chars().take(SETTINGS_COLUMNS - 2).collect();
    format!("{kept}..")
}

fn plain(kind: LineKind, text: impl Into<String>) -> Line {
    Line {
        kind,
        text: text.into(),
        focused: false,
    }
}

fn panel_row(row: PanelRow, focused: bool, text: &str) -> Line {
    let marker = if focused { "> " } else { "  " };
    Line {
        kind: LineKind::Panel(row),
        text: clip_line(format!("{marker}{text}")),
        focused,
    }
}

fn panel_lines(panel: &PanelView, page: usize, focus: usize, language: Language) -> Vec<Line> {
    let pages = page_count(panel);
    let page = page.min(pages - 1);
    let rows = page_rows(panel, page);
    let focus = focus.min(rows.len() - 1);
    let mut lines = vec![plain(LineKind::Title, tr(language, Msg::PanelTitle))];
    let waiting = panel.entries.iter().any(|entry| entry.waiting_for_consent);
    let unsaved = panel
        .entries
        .iter()
        .any(|entry| entry.kind.delivers_alerts() && entry.host.is_none());
    if panel.entries.is_empty() {
        lines.push(plain(LineKind::Info, tr(language, Msg::PanelEmpty)));
        lines.push(plain(LineKind::Info, tr(language, Msg::PanelAddHint)));
    } else if waiting {
        lines.push(plain(LineKind::Info, tr(language, Msg::PanelWaiting)));
    } else if unsaved {
        lines.push(plain(LineKind::Info, tr(language, Msg::PanelSaveHint)));
    } else {
        lines.push(plain(LineKind::Info, tr(language, Msg::PanelIntro)));
    }
    for (at, row) in rows.iter().enumerate() {
        let focused = at == focus;
        match *row {
            PanelRow::Toggle(index) => {
                let entry = &panel.entries[index];
                let state = if entry.on {
                    tr(language, Msg::On)
                } else {
                    tr(language, Msg::Off)
                };
                lines.push(panel_row(
                    *row,
                    focused,
                    &format!("[{state}] {}: {}", entry.kind.label(), entry.id),
                ));
                let status = match &entry.host {
                    Some(host) => {
                        let room =
                            SETTINGS_COLUMNS.saturating_sub(4 + entry.status.chars().count() + 3);
                        format!("    {} - {}", entry.status, host_tail(host, room))
                    }
                    None => format!("    {}", entry.status),
                };
                lines.push(plain(LineKind::Info, clip_line(status)));
            }
            PanelRow::Test(_) => {
                let text = format!("  {}", tr(language, Msg::MenuSendTest));
                lines.push(panel_row(*row, focused, &text));
            }
            PanelRow::NextPage => {
                let text = format!(
                    "{} ({} of {pages})",
                    tr(language, Msg::RowNextPage),
                    page + 1
                );
                lines.push(panel_row(*row, focused, &text));
            }
            PanelRow::Back => lines.push(panel_row(*row, focused, tr(language, Msg::RowBack))),
            PanelRow::Accept | PanelRow::Cancel => {}
        }
    }
    lines.push(plain(LineKind::Hint, tr(language, Msg::PanelHint)));
    lines
}

/// The host name as consent-page lines, or `None` when it is too long to fit.
fn consent_host_lines(entry: &PanelEntry) -> Option<Vec<String>> {
    let lines = wrap(entry.host.as_deref()?, SETTINGS_COLUMNS);
    (lines.len() <= CONSENT_HOST_LINES).then_some(lines)
}

fn consent_lines(
    panel: &PanelView,
    entry: usize,
    on_accept: bool,
    language: Language,
) -> Vec<Line> {
    let Some(shown) = panel.entries.get(entry) else {
        return Vec::new();
    };
    let mut lines = vec![plain(
        LineKind::Title,
        clip_line(format!("Turn on {}?", shown.id)),
    )];
    let host_lines = consent_host_lines(shown);
    if let Some(host_lines) = &host_lines {
        lines.push(plain(LineKind::Info, tr(language, Msg::ConsentGoesTo)));
        for line in host_lines {
            lines.push(plain(LineKind::Info, line.as_str()));
        }
        for line in wrap(what_is_sent(shown.kind).unwrap_or(""), SETTINGS_COLUMNS) {
            lines.push(plain(LineKind::Info, line));
        }
        lines.push(panel_row(
            PanelRow::Accept,
            on_accept,
            tr(language, Msg::ConsentAccept),
        ));
    } else {
        lines.push(plain(LineKind::Info, tr(language, Msg::ConsentHostTooLong)));
        lines.push(plain(
            LineKind::Info,
            clip_line(format!("stardust integration enable {}", shown.id)),
        ));
    }
    lines.push(panel_row(
        PanelRow::Cancel,
        !on_accept || host_lines.is_none(),
        tr(language, Msg::ConsentCancel),
    ));
    lines.push(plain(LineKind::Hint, tr(language, Msg::PanelHint)));
    lines
}

fn on_off(language: Language, value: bool) -> &'static str {
    if value {
        tr(language, Msg::On)
    } else {
        tr(language, Msg::Off)
    }
}

fn row_text(row: Row, view: &SettingsView, language: Language) -> String {
    match row {
        Row::Account => {
            if view.signed_in {
                tr(language, Msg::RowSignOut).to_string()
            } else {
                tr(language, Msg::RowSignIn).to_string()
            }
        }
        Row::CheckEvery => {
            let value = view
                .interval_minutes
                .map(|minutes| interval_label(language, minutes))
                .unwrap_or_else(|| tr(language, Msg::NoInterval).to_string());
            format!("{}: {value}", tr(language, Msg::RowCheckEvery))
        }
        Row::StartAtLogin => format!(
            "{}: {}",
            tr(language, Msg::RowStartAtLogin),
            on_off(language, view.start_at_login)
        ),
        Row::ShowCharacter => format!(
            "{}: {}",
            tr(language, Msg::RowShowCharacter),
            on_off(language, view.character_visible)
        ),
        Row::PetSkin => {
            let name = match view.skin {
                PetSkin::Astronaut => tr(language, Msg::SkinAstronaut),
                PetSkin::Star => tr(language, Msg::SkinStar),
            };
            format!("{}: {name}", tr(language, Msg::RowPetSkin))
        }
        Row::PetSize => format!(
            "{}: {}x",
            tr(language, Msg::RowPetSize),
            view.scale.factor()
        ),
        Row::Integrations => tr(language, Msg::RowIntegrations).to_string(),
        Row::Done => tr(language, Msg::RowDone).to_string(),
    }
}

fn next_interval(current: Option<u32>, forward: bool) -> u32 {
    let picks = QUICK_PICK_MINUTES;
    let index = current.and_then(|minutes| picks.iter().position(|pick| *pick == minutes));
    match (index, forward) {
        (Some(at), true) => picks[(at + 1) % picks.len()],
        (Some(at), false) => picks[(at + picks.len() - 1) % picks.len()],
        (None, true) => picks[0],
        (None, false) => picks[picks.len() - 1],
    }
}

fn other_skin(skin: PetSkin) -> PetSkin {
    match skin {
        PetSkin::Astronaut => PetSkin::Star,
        PetSkin::Star => PetSkin::Astronaut,
    }
}

fn other_scale(scale: PetScale) -> PetScale {
    match scale {
        PetScale::X2 => PetScale::X3,
        PetScale::X3 => PetScale::X2,
    }
}

fn activate(row: Row, view: &SettingsView, forward: bool) -> SettingsAction {
    match row {
        Row::Account => {
            if view.signed_in {
                SettingsAction::SignOut
            } else if matches!(view.sign_in, SignInStage::Waiting { .. }) {
                SettingsAction::None
            } else {
                SettingsAction::StartSignIn
            }
        }
        Row::Done => SettingsAction::Close,
        other => step(other, view, forward),
    }
}

fn step(row: Row, view: &SettingsView, forward: bool) -> SettingsAction {
    match row {
        Row::CheckEvery => {
            SettingsAction::SetInterval(next_interval(view.interval_minutes, forward))
        }
        Row::StartAtLogin if view.start_at_login_available => {
            SettingsAction::SetStartAtLogin(!view.start_at_login)
        }
        Row::ShowCharacter => SettingsAction::SetCharacterVisible(!view.character_visible),
        Row::PetSkin => SettingsAction::SetSkin(other_skin(view.skin)),
        Row::PetSize => SettingsAction::SetScale(other_scale(view.scale)),
        Row::StartAtLogin | Row::Account | Row::Integrations | Row::Done => SettingsAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Key, LineKind, Row, SettingsAction, SettingsModel, SettingsView, SignInStage, ROWS,
    };
    use crate::copy::find_disallowed_copy;
    use crate::i18n::Language;
    use crate::pet::{PetScale, PetSkin};

    fn view() -> SettingsView {
        SettingsView {
            signed_in: false,
            sign_in: SignInStage::Idle,
            interval_minutes: Some(60),
            start_at_login: false,
            start_at_login_available: true,
            character_visible: false,
            skin: PetSkin::Astronaut,
            scale: PetScale::X2,
            server_host: "helloorbit.app".to_string(),
            panel: None,
        }
    }

    #[test]
    fn every_row_is_reachable_and_changeable_with_the_keyboard_alone() {
        let view = view();
        let mut model = SettingsModel::new(true);
        let mut seen = Vec::new();
        for _ in 0..ROWS.len() {
            seen.push(model.focused());
            let action = model.handle_key(Key::Activate, &view);
            assert_ne!(action, SettingsAction::None, "{:?}", model.focused());
            assert_eq!(model.handle_key(Key::Tab, &view), SettingsAction::Moved);
        }
        assert_eq!(seen, ROWS);
        assert_eq!(model.focused(), Row::Account);
        model.handle_key(Key::BackTab, &view);
        assert_eq!(model.focused(), Row::Done);
        model.handle_key(Key::Home, &view);
        assert_eq!(model.focused(), Row::Account);
        model.handle_key(Key::End, &view);
        assert_eq!(
            model.handle_key(Key::Activate, &view),
            SettingsAction::Close
        );
        assert_eq!(model.handle_key(Key::Escape, &view), SettingsAction::Close);
    }

    #[test]
    fn rows_produce_the_expected_changes() {
        let mut view = view();
        let mut model = SettingsModel::new(false);
        assert_eq!(
            model.handle_key(Key::Activate, &view),
            SettingsAction::StartSignIn
        );
        view.sign_in = SignInStage::Waiting {
            user_code: "ABCD-EFGH".to_string(),
        };
        assert_eq!(model.handle_key(Key::Activate, &view), SettingsAction::None);
        view.signed_in = true;
        view.sign_in = SignInStage::Idle;
        assert_eq!(
            model.handle_key(Key::Activate, &view),
            SettingsAction::SignOut
        );
        model.handle_key(Key::Down, &view);
        assert_eq!(
            model.handle_key(Key::Right, &view),
            SettingsAction::SetInterval(1)
        );
        assert_eq!(
            model.handle_key(Key::Left, &view),
            SettingsAction::SetInterval(30)
        );
        view.interval_minutes = None;
        assert_eq!(
            model.handle_key(Key::Right, &view),
            SettingsAction::SetInterval(1)
        );
        model.handle_key(Key::Down, &view);
        assert_eq!(
            model.handle_key(Key::Activate, &view),
            SettingsAction::SetStartAtLogin(true)
        );
        model.handle_key(Key::Down, &view);
        assert_eq!(
            model.handle_key(Key::Activate, &view),
            SettingsAction::SetCharacterVisible(true)
        );
        model.handle_key(Key::Down, &view);
        assert_eq!(
            model.handle_key(Key::Right, &view),
            SettingsAction::SetSkin(PetSkin::Star)
        );
        model.handle_key(Key::Down, &view);
        assert_eq!(
            model.handle_key(Key::Right, &view),
            SettingsAction::SetScale(PetScale::X3)
        );
    }

    #[test]
    fn lines_mark_focus_with_text_and_show_the_user_code() {
        let mut view = view();
        view.sign_in = SignInStage::Waiting {
            user_code: "ABCD-EFGH".to_string(),
        };
        let model = SettingsModel::new(true);
        let lines = model.lines(&view, Language::English);
        assert_eq!(lines[0].text, "Welcome to Stardust");
        assert!(lines[1].text.ends_with(": ABCD-EFGH"));
        let focused: Vec<_> = lines.iter().filter(|line| line.focused).collect();
        assert_eq!(focused.len(), 1);
        assert!(focused[0].text.starts_with("> "));
        assert!(lines
            .iter()
            .filter(|line| matches!(line.kind, LineKind::Row(_)) && !line.focused)
            .all(|line| line.text.starts_with("  ")));
        for line in &lines {
            assert_eq!(find_disallowed_copy(&line.text), None, "{}", line.text);
            assert!(line.text.is_ascii(), "{}", line.text);
        }
        assert!(model
            .focus_label(&view, Language::English)
            .starts_with("Stardust settings - Sign in to ORBIT"));
    }

    #[test]
    fn a_click_focuses_and_activates_only_row_lines() {
        let view = view();
        let mut model = SettingsModel::new(false);
        assert_eq!(model.click(0, &view), SettingsAction::None);
        assert_eq!(model.click(1, &view), SettingsAction::None);
        let lines = model.lines(&view, Language::English);
        let done = lines
            .iter()
            .position(|line| line.kind == LineKind::Row(Row::Done))
            .unwrap();
        assert_eq!(model.click(done, &view), SettingsAction::Close);
        assert_eq!(model.focused(), Row::Done);
        assert_eq!(model.click(lines.len() - 1, &view), SettingsAction::None);
        assert_eq!(model.click(99, &view), SettingsAction::None);
    }

    #[test]
    fn start_at_login_row_is_hidden_when_the_platform_has_no_entry() {
        let mut view = view();
        view.start_at_login_available = false;
        let mut model = SettingsModel::new(false);
        let lines = model.lines(&view, Language::English);
        assert!(!lines
            .iter()
            .any(|line| line.kind == LineKind::Row(Row::StartAtLogin)));
        model.handle_key(Key::Down, &view);
        model.handle_key(Key::Down, &view);
        assert_eq!(model.focused(), Row::ShowCharacter);
        model.handle_key(Key::Up, &view);
        assert_eq!(model.focused(), Row::CheckEvery);
    }

    mod panel {
        use super::super::{
            panel_view, Key, Line, LineKind, PanelRow, PanelView, Row, SettingsAction,
            SettingsModel, SettingsView, PANEL_LINES, ROWS, SETTINGS_COLUMNS,
        };
        use super::view;
        use crate::consent::{Consent, CONSENT_VERSION};
        use crate::copy::find_disallowed_copy;
        use crate::delivery::{DeliveryBoard, Outcome};
        use crate::i18n::Language;
        use crate::integrations::{IntegrationKind, IntegrationRecord};

        fn record(
            kind: IntegrationKind,
            id: &str,
            host: Option<&str>,
            enabled: bool,
            accepted: bool,
        ) -> IntegrationRecord {
            let mut record = IntegrationRecord::new(kind, id).unwrap();
            record.host = host.map(str::to_string);
            record.enabled = enabled;
            if accepted {
                record.consent = host.map(|host| Consent {
                    host: host.to_string(),
                    version: CONSENT_VERSION,
                });
            }
            record
        }

        fn shown(records: &[IntegrationRecord], board: &DeliveryBoard) -> SettingsView {
            SettingsView {
                panel: Some(panel_view(records, board, Language::English)),
                ..view()
            }
        }

        fn open(view: &SettingsView) -> SettingsModel {
            let mut model = SettingsModel::new(false);
            model.open_panel();
            assert!(model.in_panel());
            let _ = view;
            model
        }

        fn lines(model: &SettingsModel, view: &SettingsView) -> Vec<Line> {
            model.lines(view, Language::English)
        }

        fn focused_text(model: &SettingsModel, view: &SettingsView) -> String {
            let found = lines(model, view);
            let focused: Vec<&Line> = found.iter().filter(|line| line.focused).collect();
            assert_eq!(focused.len(), 1, "{found:?}");
            assert!(focused[0].text.starts_with("> "), "{found:?}");
            focused[0].text.clone()
        }

        fn focus_on(model: &mut SettingsModel, view: &SettingsView, wanted: &str) {
            for _ in 0..40 {
                if focused_text(model, view).contains(wanted) {
                    return;
                }
                model.handle_key(Key::Down, view);
            }
            panic!("no row with {wanted}: {:?}", lines(model, view));
        }

        fn assert_fits(lines: &[Line]) {
            assert!(lines.len() <= PANEL_LINES, "{} lines", lines.len());
            for line in lines {
                let count = line.text.chars().count();
                assert!(count <= SETTINGS_COLUMNS, "{count}: {}", line.text);
                assert!(line.text.is_ascii(), "{}", line.text);
                assert_eq!(find_disallowed_copy(&line.text), None, "{}", line.text);
            }
        }

        #[test]
        fn the_row_appears_only_with_the_panel_on_and_opens_by_key_and_click() {
            assert_eq!(super::super::visible_rows(&view()), ROWS);
            let on = shown(&[], &DeliveryBoard::default());
            let rows = super::super::visible_rows(&on);
            assert_eq!(rows.len(), ROWS.len() + 1);
            assert_eq!(rows[rows.len() - 2], Row::Integrations);
            assert_eq!(rows[rows.len() - 1], Row::Done);

            let mut model = SettingsModel::new(false);
            for _ in 0..rows.len() - 2 {
                model.handle_key(Key::Down, &on);
            }
            assert_eq!(model.focused(), Row::Integrations);
            assert_eq!(
                model.handle_key(Key::Right, &on),
                SettingsAction::None,
                "not a choice to step through"
            );
            assert!(!model.in_panel());
            assert_eq!(model.handle_key(Key::Activate, &on), SettingsAction::Moved);
            assert!(model.in_panel());
            assert_eq!(model.handle_key(Key::Escape, &on), SettingsAction::Moved);
            assert!(!model.in_panel(), "Esc goes back rather than closing");
            assert_eq!(model.handle_key(Key::Escape, &on), SettingsAction::Close);

            let mut clicked = SettingsModel::new(false);
            let main = lines(&clicked, &on);
            let row = main
                .iter()
                .position(|line| line.kind == LineKind::Row(Row::Integrations))
                .unwrap();
            assert_eq!(main[row].text, "  Integrations...");
            assert_eq!(clicked.click(row, &on), SettingsAction::Moved);
            assert!(clicked.in_panel());
        }

        #[test]
        fn each_entry_shows_kind_name_state_host_and_last_result() {
            let mut board = DeliveryBoard::default();
            board.record("ops", Outcome::RetryPending);
            let records = [record(
                IntegrationKind::SlackIncoming,
                "ops",
                Some("hooks.slack.com"),
                true,
                true,
            )];
            let on = shown(&records, &board);
            let model = open(&on);
            let found = lines(&model, &on);
            assert_eq!(found[0].text, "Integrations");
            assert_eq!(found[2].text, "> [on] Slack: ops");
            assert_eq!(found[3].text, "    Retry pending - hooks.slack.com");
            assert_eq!(found[4].text, "    Send test");
            assert_eq!(found[5].text, "  Back");
            assert_eq!(found[6].kind, LineKind::Hint);
            assert_fits(&found);
            let label = model.focus_label(&on, Language::English);
            assert!(label.contains("[on] Slack: ops"), "{label}");
            assert!(label.contains("Retry pending"), "{label}");
        }

        #[test]
        fn turning_on_asks_first_with_cancel_focused_and_states_what_is_sent() {
            let records = [record(
                IntegrationKind::SlackIncoming,
                "ops",
                Some("hooks.slack.com"),
                false,
                false,
            )];
            let on = shown(&records, &DeliveryBoard::default());
            let mut model = open(&on);
            assert_eq!(model.handle_key(Key::Activate, &on), SettingsAction::Moved);
            let found = lines(&model, &on);
            assert_eq!(found[0].text, "Turn on ops?");
            let text: Vec<&str> = found.iter().map(|line| line.text.as_str()).collect();
            assert!(text.contains(&"Each alert goes to:"), "{text:?}");
            assert!(text.contains(&"hooks.slack.com"), "{text:?}");
            assert!(
                text.iter()
                    .any(|line| line.contains("Alert title, severity")),
                "{text:?}"
            );
            assert!(focused_text(&model, &on).contains("Cancel"));
            assert_fits(&found);
            let label = model.focus_label(&on, Language::English);
            assert!(label.contains("hooks.slack.com"), "{label}");
            assert!(label.contains("Nothing else"), "{label}");

            // Enter again does not accept.
            assert_eq!(model.handle_key(Key::Activate, &on), SettingsAction::Moved);
            assert_eq!(lines(&model, &on)[0].text, "Integrations");

            model.handle_key(Key::Activate, &on);
            model.handle_key(Key::Escape, &on);
            assert_eq!(lines(&model, &on)[0].text, "Integrations", "Esc cancels");

            model.handle_key(Key::Activate, &on);
            model.handle_key(Key::Up, &on);
            assert!(focused_text(&model, &on).contains("Accept"));
            let accepted = SettingsAction::AcceptConsent {
                id: "ops".to_string(),
                host: "hooks.slack.com".to_string(),
            };
            assert_eq!(model.handle_key(Key::Activate, &on), accepted);
            assert_eq!(lines(&model, &on)[0].text, "Integrations");

            model.handle_key(Key::Activate, &on);
            let page = lines(&model, &on);
            let accept = page
                .iter()
                .position(|line| line.kind == LineKind::Panel(PanelRow::Accept))
                .unwrap();
            assert_eq!(
                model.click(accept, &on),
                SettingsAction::Moved,
                "the first click only focuses Accept"
            );
            assert!(focused_text(&model, &on).contains("Accept"));
            assert_eq!(model.click(accept, &on), accepted, "the mouse can accept");
            model.handle_key(Key::Activate, &on);
            let page = lines(&model, &on);
            let cancel = page
                .iter()
                .position(|line| line.kind == LineKind::Panel(PanelRow::Cancel))
                .unwrap();
            assert_eq!(model.click(cancel, &on), SettingsAction::Moved);
            assert_eq!(model.click(0, &on), SettingsAction::None);
        }

        /// A double-click is two clicks at one place. The first opens the
        /// consent page; wherever the second lands, it must not accept.
        #[test]
        fn a_double_click_that_opens_the_consent_page_never_accepts() {
            let records = [
                record(
                    IntegrationKind::SlackIncoming,
                    "first",
                    Some("hooks.slack.com"),
                    true,
                    true,
                ),
                record(
                    IntegrationKind::TeamsIncoming,
                    "second",
                    Some("prod-00.westus.logic.azure.com"),
                    false,
                    false,
                ),
                record(
                    IntegrationKind::GenericJson,
                    "third",
                    Some("hooks.example"),
                    false,
                    false,
                ),
            ];
            let view = shown(&records, &DeliveryBoard::default());
            let panel_rows: Vec<usize> = {
                let model = open(&view);
                lines(&model, &view)
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| matches!(line.kind, LineKind::Panel(_)))
                    .map(|(at, _)| at)
                    .collect()
            };
            for at in panel_rows {
                let mut model = open(&view);
                let first = model.click(at, &view);
                let second = model.click(at, &view);
                for action in [first, second] {
                    assert!(
                        !matches!(action, SettingsAction::AcceptConsent { .. }),
                        "line {at}"
                    );
                }
            }
        }

        #[test]
        fn an_accepted_integration_toggles_and_tests_without_asking_again() {
            let records = [record(
                IntegrationKind::TeamsIncoming,
                "team",
                Some("prod-00.westus.logic.azure.com"),
                false,
                true,
            )];
            let off = shown(&records, &DeliveryBoard::default());
            let mut model = open(&off);
            assert_eq!(
                model.handle_key(Key::Activate, &off),
                SettingsAction::SetIntegration {
                    id: "team".to_string(),
                    on: true
                }
            );
            let mut enabled = records.clone();
            enabled[0].enabled = true;
            let on = shown(&enabled, &DeliveryBoard::default());
            assert_eq!(
                model.handle_key(Key::Activate, &on),
                SettingsAction::SetIntegration {
                    id: "team".to_string(),
                    on: false
                }
            );
            focus_on(&mut model, &on, "Send test");
            assert_eq!(
                model.handle_key(Key::Activate, &on),
                SettingsAction::SendTest {
                    id: "team".to_string()
                }
            );
            focus_on(&mut model, &on, "Back");
            assert_eq!(model.handle_key(Key::Activate, &on), SettingsAction::Moved);
            assert!(!model.in_panel());
        }

        #[test]
        fn an_enabled_integration_without_consent_reads_as_off_and_says_why() {
            let records = [
                record(
                    IntegrationKind::SlackIncoming,
                    "ops",
                    Some("hooks.slack.com"),
                    true,
                    false,
                ),
                record(
                    IntegrationKind::SlackIncoming,
                    "moved",
                    Some("hooks.slack-gov.com"),
                    true,
                    false,
                ),
            ];
            let mut records = records;
            records[1].consent = Some(Consent {
                host: "hooks.slack.com".to_string(),
                version: CONSENT_VERSION,
            });
            let view = shown(&records, &DeliveryBoard::default());
            let mut model = open(&view);
            let found = lines(&model, &view);
            assert_eq!(
                found[1].text,
                "Some wait for your consent and send nothing yet"
            );
            assert_eq!(found[2].text, "> [off] Slack: ops");
            assert_eq!(found[3].text, "    Needs consent - hooks.slack.com");
            assert_eq!(found[6].text, "    Needs consent - hooks.slack-gov.com");
            focus_on(&mut model, &view, "Send test");
            assert_eq!(
                model.handle_key(Key::Activate, &view),
                SettingsAction::Moved,
                "a test asks first too"
            );
            assert_eq!(lines(&model, &view)[0].text, "Turn on ops?");
        }

        #[test]
        fn mcp_shows_no_alert_delivery_and_no_test_or_consent() {
            let records = [record(
                IntegrationKind::OrbitMcp,
                "orbit",
                Some("helloorbit.app"),
                false,
                false,
            )];
            let off = shown(&records, &DeliveryBoard::default());
            let mut model = open(&off);
            let found = lines(&model, &off);
            assert_eq!(found[2].text, "> [off] ORBIT MCP: orbit");
            assert_eq!(found[3].text, "    No alert delivery - helloorbit.app");
            assert!(!found.iter().any(|line| line.text.contains("Send test")));
            assert_eq!(
                model.handle_key(Key::Activate, &off),
                SettingsAction::SetIntegration {
                    id: "orbit".to_string(),
                    on: true
                },
                "nothing to accept: no alert goes there"
            );
            let slack = [record(
                IntegrationKind::SlackMcp,
                "slack",
                None,
                true,
                false,
            )];
            let view = shown(&slack, &DeliveryBoard::default());
            let found = lines(&open(&view), &view);
            assert_eq!(found[3].text, "    No alert delivery");
        }

        #[test]
        fn without_a_destination_nothing_can_be_accepted_and_the_page_says_how() {
            let records = [record(
                IntegrationKind::SlackIncoming,
                "ops",
                None,
                false,
                false,
            )];
            let view = shown(&records, &DeliveryBoard::default());
            let mut model = open(&view);
            let found = lines(&model, &view);
            assert_eq!(
                found[1].text,
                "Save a destination: stardust integration url ID URL"
            );
            assert_eq!(found[3].text, "    No destination saved");
            assert!(!found.iter().any(|line| line.text.contains("Send test")));
            assert_eq!(model.handle_key(Key::Activate, &view), SettingsAction::None);
            assert_eq!(lines(&model, &view)[0].text, "Integrations");
            let empty = shown(&[], &DeliveryBoard::default());
            let found = lines(&open(&empty), &empty);
            assert_eq!(found[1].text, "No integrations yet");
            assert!(found[2].text.contains("integration add"));
            assert_fits(&found);
        }

        /// Sixteen is the most a config can hold. Every one must be reachable
        /// and every page must fit the window, with the longest names.
        #[test]
        fn pages_reach_every_integration_and_always_fit_the_window() {
            let host = format!("{}.example", "h".repeat(240));
            let mut records = Vec::new();
            for index in 0..16 {
                let id = format!("{index:02}-{}", "x".repeat(29));
                assert_eq!(id.len(), 32);
                records.push(record(
                    IntegrationKind::GenericJson,
                    &id,
                    Some(host.as_str()),
                    true,
                    true,
                ));
            }
            let mut board = DeliveryBoard::default();
            for one in &records {
                board.record(&one.id, Outcome::GoneDisabled);
            }
            let view = shown(&records, &board);
            let mut model = open(&view);
            let mut seen = Vec::new();
            for page in 1..=6 {
                let found = lines(&model, &view);
                assert_fits(&found);
                for line in &found {
                    if let LineKind::Panel(PanelRow::Toggle(index)) = line.kind {
                        seen.push(index);
                    }
                }
                assert!(
                    found
                        .iter()
                        .any(|line| line.text.contains(&format!("Next page ({page} of 6)"))),
                    "{found:?}"
                );
                focus_on(&mut model, &view, "Next page");
                assert_eq!(
                    model.handle_key(Key::Activate, &view),
                    SettingsAction::Moved
                );
            }
            assert_eq!(seen, (0..16).collect::<Vec<_>>());
            assert!(
                lines(&model, &view)
                    .iter()
                    .any(|line| line.text.contains("(1 of 6)")),
                "wraps to the first page"
            );
            let status = lines(&model, &view)
                .iter()
                .find(|line| line.text.contains("destination gone"))
                .unwrap()
                .text
                .clone();
            assert!(status.ends_with(".example"), "keeps the end: {status}");
            model.handle_key(Key::Right, &view);
            assert!(lines(&model, &view)
                .iter()
                .any(|line| line.text.contains("(2 of 6)")));
            model.handle_key(Key::Left, &view);
            model.handle_key(Key::Left, &view);
            assert!(lines(&model, &view)
                .iter()
                .any(|line| line.text.contains("(6 of 6)")));
            let single = shown(&records[..1], &board);
            let mut model = open(&single);
            assert_eq!(model.handle_key(Key::Right, &single), SettingsAction::None);
        }

        #[test]
        fn the_longest_consent_page_fits_and_a_longer_host_goes_to_the_command_line() {
            let fits = "h".repeat(SETTINGS_COLUMNS * 4 - 8);
            for kind in [
                IntegrationKind::GenericJson,
                IntegrationKind::PagerDuty,
                IntegrationKind::SlackIncoming,
                IntegrationKind::TeamsIncoming,
                IntegrationKind::DiscordWebhook,
            ] {
                let host = format!("{fits}.example");
                let records = [record(kind, "ops", Some(host.as_str()), false, false)];
                let view = shown(&records, &DeliveryBoard::default());
                let mut model = open(&view);
                model.handle_key(Key::Activate, &view);
                let found = lines(&model, &view);
                assert_fits(&found);
                let joined: String = found.iter().map(|line| line.text.as_str()).collect();
                assert!(joined.contains(&host), "the whole host: {joined}");
                assert!(found
                    .iter()
                    .any(|line| line.kind == LineKind::Panel(PanelRow::Accept)));
            }
            let host = format!("{}.example", "h".repeat(240));
            let records = [record(
                IntegrationKind::SlackIncoming,
                "ops",
                Some(host.as_str()),
                false,
                false,
            )];
            let view = shown(&records, &DeliveryBoard::default());
            let mut model = open(&view);
            model.handle_key(Key::Activate, &view);
            let found = lines(&model, &view);
            assert_fits(&found);
            assert!(found
                .iter()
                .any(|line| line.text == "stardust integration enable ops"));
            assert!(!found
                .iter()
                .any(|line| line.kind == LineKind::Panel(PanelRow::Accept)));
            model.handle_key(Key::Up, &view);
            assert!(focused_text(&model, &view).contains("Cancel"));
            assert_eq!(
                model.handle_key(Key::Activate, &view),
                SettingsAction::Moved,
                "cannot accept a host it could not show"
            );
        }

        #[test]
        fn nothing_on_either_page_looks_like_a_url_or_a_secret() {
            let mut records = vec![
                record(
                    IntegrationKind::GenericJson,
                    "json",
                    Some("hooks.example"),
                    true,
                    false,
                ),
                record(
                    IntegrationKind::OrbitMcp,
                    "orbit",
                    Some("helloorbit.app"),
                    true,
                    false,
                ),
            ];
            records[1].endpoint = Some("https://helloorbit.app/mcp/private-path".to_string());
            records[1].client_id = Some("client-id-value".to_string());
            let view = shown(&records, &DeliveryBoard::default());
            let debug = format!("{:?}", view.panel);
            assert!(!debug.contains("private-path"), "{debug}");
            assert!(!debug.contains("client-id-value"), "{debug}");
            let mut model = open(&view);
            let mut pages = lines(&model, &view);
            model.handle_key(Key::Activate, &view);
            pages.extend(lines(&model, &view));
            for line in pages {
                assert!(!line.text.contains("://"), "{}", line.text);
                assert!(!line.text.contains("private-path"), "{}", line.text);
            }
        }

        #[test]
        fn a_page_with_no_panel_behind_it_falls_back_to_the_main_rows() {
            let mut model = SettingsModel::new(false);
            model.open_panel();
            let plain = view();
            assert_eq!(model.handle_key(Key::Down, &plain), SettingsAction::Moved);
            assert!(!model.in_panel());
            assert_eq!(
                panel_view(&[], &DeliveryBoard::default(), Language::English),
                PanelView::default()
            );
        }
    }
}
