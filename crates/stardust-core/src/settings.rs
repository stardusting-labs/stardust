//! Settings and first-run window model.
//!
//! The window is drawn by the app. This module owns the rows, keyboard
//! focus, and what each key or click does, so the behaviour is testable
//! without a display. Every row is reachable with the keyboard alone:
//! Up/Down or Tab/Shift+Tab move, Enter or Space changes, Left/Right step a
//! choice, Esc closes. The focused row is marked with `>` as well as by
//! colour, so focus does not depend on colour alone.

use crate::i18n::{interval_label, tr, Language, Msg};
use crate::pet::{PetScale, PetSkin};
use crate::schedule::QUICK_PICK_MINUTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Account,
    CheckEvery,
    StartAtLogin,
    ShowCharacter,
    PetSkin,
    PetSize,
    Language,
    Done,
}

pub const ROWS: [Row; 8] = [
    Row::Account,
    Row::CheckEvery,
    Row::StartAtLogin,
    Row::ShowCharacter,
    Row::PetSkin,
    Row::PetSize,
    Row::Language,
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
    pub language: Language,
    /// The ORBIT host shown on the account row, without a scheme.
    pub server_host: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    SetLanguage(Language),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Title,
    Info,
    Row(Row),
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsModel {
    focus: Row,
    first_run: bool,
}

/// Rows shown for this view. The start-at-login row is left out where the
/// platform has no entry for it.
pub fn visible_rows(view: &SettingsView) -> Vec<Row> {
    ROWS.into_iter()
        .filter(|row| *row != Row::StartAtLogin || view.start_at_login_available)
        .collect()
}

impl SettingsModel {
    pub fn new(first_run: bool) -> Self {
        Self {
            focus: Row::Account,
            first_run,
        }
    }

    pub fn focused(&self) -> Row {
        self.focus
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
        match key {
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
        let Some(LineKind::Row(row)) = lines.get(line_index).map(|line| line.kind) else {
            return SettingsAction::None;
        };
        self.focus = row;
        activate(row, view, true)
    }

    pub fn lines(&self, view: &SettingsView, language: Language) -> Vec<Line> {
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
        format!(
            "{} - {}",
            tr(language, Msg::SettingsTitle),
            row_text(self.focused(), view, language)
        )
    }
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
        Row::Language => format!(
            "{}: {}",
            tr(language, Msg::RowLanguage),
            view.language.native_name()
        ),
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
        Row::Language => SettingsAction::SetLanguage(view.language.next()),
        Row::StartAtLogin | Row::Account | Row::Done => SettingsAction::None,
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
            language: Language::English,
            server_host: "helloorbit.app".to_string(),
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
        model.handle_key(Key::Down, &view);
        assert_eq!(
            model.handle_key(Key::Activate, &view),
            SettingsAction::SetLanguage(Language::Korean)
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
        for language in Language::ALL {
            for line in model.lines(&view, language) {
                assert_eq!(find_disallowed_copy(&line.text), None, "{}", line.text);
            }
        }
        let korean = model.lines(&view, Language::Korean);
        assert_eq!(korean[0].text, "Stardust에 오신 것을 환영해요");
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
}
