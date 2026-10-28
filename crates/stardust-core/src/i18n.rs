//! English and Korean interface text.
//!
//! The tray menu, OS notifications, and window titles are drawn by the OS, so
//! Hangul shows there with the system font. The speech bubble and the
//! settings window draw their own letters. They use Korean only when a Hangul
//! font is available to the app (see [`drawn_language`]); otherwise they fall
//! back to English so no empty boxes are shown.
//!
//! TODO(Unsu): the Korean strings need a native-speaker review before release.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
    Korean,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::Korean];

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Korean => "ko",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let lower = code.trim().to_ascii_lowercase();
        let primary = lower.split(['_', '-', '.', '@']).next().unwrap_or_default();
        match primary {
            "en" => Some(Self::English),
            "ko" => Some(Self::Korean),
            _ => None,
        }
    }

    /// The language's own name, for the picker.
    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Korean => "한국어",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::English => Self::Korean,
            Self::Korean => Self::English,
        }
    }
}

/// Pick the language: the saved choice first, then `LC_ALL`, `LC_MESSAGES`,
/// `LANG` in POSIX order, then English.
///
/// Windows and macOS usually do not set these variables for GUI apps, so the
/// saved choice is the reliable path there.
pub fn detect(saved: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Language {
    if let Some(language) = saved.and_then(Language::from_code) {
        return language;
    }
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(value) = env(name).filter(|value| !value.trim().is_empty()) {
            return Language::from_code(&value).unwrap_or_default();
        }
    }
    Language::English
}

/// Language for text the app draws itself.
pub fn drawn_language(language: Language, has_hangul_font: bool) -> Language {
    match language {
        Language::Korean if !has_hangul_font => Language::English,
        other => other,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    MenuPet,
    MenuCheckNow,
    MenuQuit,
    MenuSettings,
    MenuSignIn,
    ShowCharacter,
    HideCharacter,
    NoteUnavailable,
    NoteSignedOut,
    RetryLater,
    SignInHint,
    SettingsTitle,
    Welcome,
    RowSignIn,
    RowSignOut,
    RowCheckEvery,
    RowStartAtLogin,
    RowShowCharacter,
    RowPetSkin,
    RowPetSize,
    RowLanguage,
    RowDone,
    On,
    Off,
    SignedIn,
    NotSignedIn,
    WaitingForApproval,
    SignInFailed,
    KeysHint,
    SkinAstronaut,
    SkinStar,
    NoInterval,
}

pub const ALL_MSGS: [Msg; 32] = [
    Msg::MenuPet,
    Msg::MenuCheckNow,
    Msg::MenuQuit,
    Msg::MenuSettings,
    Msg::MenuSignIn,
    Msg::ShowCharacter,
    Msg::HideCharacter,
    Msg::NoteUnavailable,
    Msg::NoteSignedOut,
    Msg::RetryLater,
    Msg::SignInHint,
    Msg::SettingsTitle,
    Msg::Welcome,
    Msg::RowSignIn,
    Msg::RowSignOut,
    Msg::RowCheckEvery,
    Msg::RowStartAtLogin,
    Msg::RowShowCharacter,
    Msg::RowPetSkin,
    Msg::RowPetSize,
    Msg::RowLanguage,
    Msg::RowDone,
    Msg::On,
    Msg::Off,
    Msg::SignedIn,
    Msg::NotSignedIn,
    Msg::WaitingForApproval,
    Msg::SignInFailed,
    Msg::KeysHint,
    Msg::SkinAstronaut,
    Msg::SkinStar,
    Msg::NoInterval,
];

pub fn tr(language: Language, msg: Msg) -> &'static str {
    match language {
        Language::English => english(msg),
        Language::Korean => korean(msg),
    }
}

fn english(msg: Msg) -> &'static str {
    match msg {
        Msg::MenuPet => "Pet",
        Msg::MenuCheckNow => "Check now",
        Msg::MenuQuit => "Quit",
        Msg::MenuSettings => "Settings...",
        Msg::MenuSignIn => "Sign in...",
        Msg::ShowCharacter => "Show character",
        Msg::HideCharacter => "Hide character",
        Msg::NoteUnavailable => "ORBIT is not available right now",
        Msg::NoteSignedOut => "Sign in to see ORBIT items",
        Msg::RetryLater => "Stardust will try again later",
        Msg::SignInHint => "Choose Sign in from the tray menu",
        Msg::SettingsTitle => "Stardust settings",
        Msg::Welcome => "Welcome to Stardust",
        Msg::RowSignIn => "Sign in to ORBIT",
        Msg::RowSignOut => "Sign out",
        Msg::RowCheckEvery => "Check every",
        Msg::RowStartAtLogin => "Start at login",
        Msg::RowShowCharacter => "Show character",
        Msg::RowPetSkin => "Pet",
        Msg::RowPetSize => "Size",
        Msg::RowLanguage => "Language",
        Msg::RowDone => "Done",
        Msg::On => "on",
        Msg::Off => "off",
        Msg::SignedIn => "Signed in",
        Msg::NotSignedIn => "Not signed in",
        Msg::WaitingForApproval => "Enter this code on the ORBIT account page",
        Msg::SignInFailed => "Sign-in did not finish. Try again",
        Msg::KeysHint => "Arrows move. Enter changes. Esc closes",
        Msg::SkinAstronaut => "Astronaut",
        Msg::SkinStar => "Star",
        Msg::NoInterval => "set times only",
    }
}

fn korean(msg: Msg) -> &'static str {
    match msg {
        Msg::MenuPet => "펫",
        Msg::MenuCheckNow => "지금 확인",
        Msg::MenuQuit => "종료",
        Msg::MenuSettings => "설정...",
        Msg::MenuSignIn => "로그인...",
        Msg::ShowCharacter => "캐릭터 보이기",
        Msg::HideCharacter => "캐릭터 숨기기",
        Msg::NoteUnavailable => "지금은 ORBIT에 연결할 수 없어요",
        Msg::NoteSignedOut => "ORBIT 항목을 보려면 로그인하세요",
        Msg::RetryLater => "잠시 후 다시 시도할게요",
        Msg::SignInHint => "트레이 메뉴에서 로그인을 고르세요",
        Msg::SettingsTitle => "Stardust 설정",
        Msg::Welcome => "Stardust에 오신 것을 환영해요",
        Msg::RowSignIn => "ORBIT 로그인",
        Msg::RowSignOut => "로그아웃",
        Msg::RowCheckEvery => "확인 간격",
        Msg::RowStartAtLogin => "로그인할 때 시작",
        Msg::RowShowCharacter => "캐릭터 보이기",
        Msg::RowPetSkin => "펫",
        Msg::RowPetSize => "크기",
        Msg::RowLanguage => "언어",
        Msg::RowDone => "완료",
        Msg::On => "켬",
        Msg::Off => "끔",
        Msg::SignedIn => "로그인됨",
        Msg::NotSignedIn => "로그인 안 됨",
        Msg::WaitingForApproval => "ORBIT 계정 페이지에 이 코드를 입력하세요",
        Msg::SignInFailed => "로그인을 마치지 못했어요. 다시 시도하세요",
        Msg::KeysHint => "화살표로 이동, Enter로 변경, Esc로 닫기",
        Msg::SkinAstronaut => "우주인",
        Msg::SkinStar => "별",
        Msg::NoInterval => "정한 시각에만",
    }
}

/// An interval length: "30 minutes" or "30분".
pub fn interval_label(language: Language, minutes: u32) -> String {
    match language {
        Language::English => crate::schedule::format_interval(minutes),
        Language::Korean => format!("{minutes}분"),
    }
}

/// "Every 30 minutes" in the tray menu.
pub fn every(language: Language, interval_label: &str) -> String {
    match language {
        Language::English => format!("Every {interval_label}"),
        Language::Korean => format!("{interval_label}마다"),
    }
}

#[cfg(test)]
mod tests {
    use super::{detect, drawn_language, every, tr, Language, ALL_MSGS};
    use crate::copy::find_disallowed_copy;

    fn has_hangul(text: &str) -> bool {
        text.chars()
            .any(|ch| ('\u{AC00}'..='\u{D7A3}').contains(&ch))
    }

    #[test]
    fn every_message_has_english_and_korean_text() {
        for msg in ALL_MSGS {
            let en = tr(Language::English, msg);
            let ko = tr(Language::Korean, msg);
            assert!(!en.is_empty(), "{msg:?}");
            assert!(!ko.is_empty(), "{msg:?}");
            assert!(en.is_ascii(), "{msg:?}");
            assert!(has_hangul(ko), "{msg:?} has no Hangul");
            assert_eq!(find_disallowed_copy(en), None, "{msg:?}");
            assert_eq!(find_disallowed_copy(ko), None, "{msg:?}");
        }
        assert_eq!(every(Language::English, "30 min"), "Every 30 min");
        assert!(has_hangul(&every(Language::Korean, "30분")));
        assert_eq!(
            every(
                Language::Korean,
                &super::interval_label(Language::Korean, 30)
            ),
            "30분마다"
        );
        assert_eq!(super::interval_label(Language::English, 1), "1 minute");
    }

    #[test]
    fn saved_choice_then_posix_locale_then_english() {
        let korean_env = |name: &str| (name == "LANG").then(|| "ko_KR.UTF-8".to_string());
        let empty_env = |_: &str| None;
        let override_env = |name: &str| match name {
            "LC_ALL" => Some("en_US.UTF-8".to_string()),
            "LANG" => Some("ko_KR.UTF-8".to_string()),
            _ => None,
        };
        assert_eq!(detect(None, &korean_env), Language::Korean);
        assert_eq!(detect(Some("en"), &korean_env), Language::English);
        assert_eq!(detect(Some("ko"), &empty_env), Language::Korean);
        assert_eq!(detect(Some("fr"), &empty_env), Language::English);
        assert_eq!(detect(None, &override_env), Language::English);
        assert_eq!(detect(None, &empty_env), Language::English);
        assert_eq!(Language::from_code("ko-KR"), Some(Language::Korean));
        assert_eq!(Language::from_code("C"), None);
    }

    #[test]
    fn drawn_text_falls_back_to_english_without_a_hangul_font() {
        assert_eq!(drawn_language(Language::Korean, false), Language::English);
        assert_eq!(drawn_language(Language::Korean, true), Language::Korean);
        assert_eq!(drawn_language(Language::English, true), Language::English);
        assert_eq!(Language::English.next(), Language::Korean);
        assert_eq!(Language::Korean.native_name(), "한국어");
    }
}
