//! Interface text.
//!
//! Stardust is English only. Every user-facing string goes through [`tr`] so
//! the wording lives in one place.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
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
    ConnectConnecting,
    ConnectConnected,
    ConnectSignedOut,
    ConnectOffline,
    MenuSendTest,
    TestSent,
    TestFailed,
    MenuSignOut,
    SignInWindowName,
    SignInTitle,
    SignInRequesting,
    SignInEnterCode,
    SignInCheckYouStarted,
    SignInOpenAccountPage,
    SignInAddressTooLong,
    SignInExpiresIn,
    SignInWaiting,
    SignInReconnecting,
    SignInBrowserFailed,
    SignInBrowserFailedNoAddress,
    SignInCopied,
    SignInCopyFailed,
    SignInSignedInTo,
    SignInDenied,
    SignInExpired,
    SignInOffline,
    SignInUnavailable,
    SignInKeychainFailed,
    SignInKeychainUnreadable,
    SignInRowOpen,
    SignInRowCopy,
    SignInRowCancel,
    SignInRowTryAgain,
    SignInRowNewCode,
    SignInRowNotNow,
    SignInRowClose,
    SignInKeysCancel,
    SignInKeysClose,
    SignInKeysRequesting,
    SignOutFailed,
    SignOutFailedHint,
}

pub const ALL_MSGS: [Msg; 72] = [
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
    Msg::ConnectConnecting,
    Msg::ConnectConnected,
    Msg::ConnectSignedOut,
    Msg::ConnectOffline,
    Msg::MenuSendTest,
    Msg::TestSent,
    Msg::TestFailed,
    Msg::MenuSignOut,
    Msg::SignInWindowName,
    Msg::SignInTitle,
    Msg::SignInRequesting,
    Msg::SignInEnterCode,
    Msg::SignInCheckYouStarted,
    Msg::SignInOpenAccountPage,
    Msg::SignInAddressTooLong,
    Msg::SignInExpiresIn,
    Msg::SignInWaiting,
    Msg::SignInReconnecting,
    Msg::SignInBrowserFailed,
    Msg::SignInBrowserFailedNoAddress,
    Msg::SignInCopied,
    Msg::SignInCopyFailed,
    Msg::SignInSignedInTo,
    Msg::SignInDenied,
    Msg::SignInExpired,
    Msg::SignInOffline,
    Msg::SignInUnavailable,
    Msg::SignInKeychainFailed,
    Msg::SignInKeychainUnreadable,
    Msg::SignInRowOpen,
    Msg::SignInRowCopy,
    Msg::SignInRowCancel,
    Msg::SignInRowTryAgain,
    Msg::SignInRowNewCode,
    Msg::SignInRowNotNow,
    Msg::SignInRowClose,
    Msg::SignInKeysCancel,
    Msg::SignInKeysClose,
    Msg::SignInKeysRequesting,
    Msg::SignOutFailed,
    Msg::SignOutFailedHint,
];

pub fn tr(language: Language, msg: Msg) -> &'static str {
    match language {
        Language::English => english(msg),
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
        Msg::ConnectConnecting => "ORBIT: Connecting...",
        Msg::ConnectConnected => "ORBIT: Connected",
        Msg::ConnectSignedOut => "ORBIT: Signed out",
        Msg::ConnectOffline => "ORBIT: Offline",
        Msg::MenuSendTest => "Send test",
        Msg::TestSent => "Test sent",
        Msg::TestFailed => "Test did not go through",
        Msg::MenuSignOut => "Sign out",
        Msg::SignInWindowName => "Stardust sign-in",
        Msg::SignInTitle => "Sign in to ORBIT",
        Msg::SignInRequesting => "Asking ORBIT for a code...",
        Msg::SignInEnterCode => "Enter code",
        Msg::SignInCheckYouStarted => "Only enter this code if you started this sign-in.",
        Msg::SignInOpenAccountPage => "Open the ORBIT account page in your browser.",
        Msg::SignInAddressTooLong => "The address is too long to show. Use the Open row.",
        Msg::SignInExpiresIn => "Code expires in",
        Msg::SignInWaiting => "Waiting for approval...",
        Msg::SignInReconnecting => "Cannot reach ORBIT. Still trying",
        Msg::SignInBrowserFailed => "Could not open a browser. Type the address above",
        Msg::SignInBrowserFailedNoAddress => "No browser opened. Go to the ORBIT account page",
        Msg::SignInCopied => "Code copied",
        Msg::SignInCopyFailed => "Could not copy. Type the code instead",
        Msg::SignInSignedInTo => "Signed in to",
        Msg::SignInDenied => "ORBIT denied this sign-in",
        Msg::SignInExpired => "This code expired",
        Msg::SignInOffline => "Cannot reach ORBIT",
        Msg::SignInUnavailable => "Sign-in is not available on this server",
        Msg::SignInKeychainFailed => {
            "ORBIT approved, but the system keychain did not save the sign-in"
        }
        Msg::SignInKeychainUnreadable => {
            "Stardust could not read the system keychain. Unlock it and try again"
        }
        Msg::SignInRowOpen => "Open ORBIT in browser",
        Msg::SignInRowCopy => "Copy code",
        Msg::SignInRowCancel => "Cancel",
        Msg::SignInRowTryAgain => "Try again",
        Msg::SignInRowNewCode => "Get a new code",
        Msg::SignInRowNotNow => "Not now",
        Msg::SignInRowClose => "Close",
        Msg::SignInKeysCancel => "Arrows move. Enter chooses. Esc cancels",
        Msg::SignInKeysClose => "Arrows move. Enter chooses. Esc closes",
        Msg::SignInKeysRequesting => "Esc cancels",
        Msg::SignOutFailed => "Could not sign out",
        Msg::SignOutFailedHint => "The keychain still holds the sign-in",
    }
}

/// An interval length: "30 minutes".
pub fn interval_label(language: Language, minutes: u32) -> String {
    match language {
        Language::English => crate::schedule::format_interval(minutes),
    }
}

/// "Every 30 minutes" in the tray menu.
pub fn every(language: Language, interval_label: &str) -> String {
    match language {
        Language::English => format!("Every {interval_label}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{every, tr, Language, ALL_MSGS};
    use crate::copy::find_disallowed_copy;

    #[test]
    fn every_message_has_english_text() {
        for msg in ALL_MSGS {
            let en = tr(Language::English, msg);
            assert!(!en.is_empty(), "{msg:?}");
            assert!(en.is_ascii(), "{msg:?}");
            assert_eq!(find_disallowed_copy(en), None, "{msg:?}");
        }
        assert_eq!(every(Language::English, "30 min"), "Every 30 min");
        assert_eq!(super::interval_label(Language::English, 1), "1 minute");
    }
}
