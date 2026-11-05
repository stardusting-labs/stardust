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
    RowIntegrations,
    PanelTitle,
    PanelIntro,
    PanelWaiting,
    PanelEmpty,
    PanelAddHint,
    PanelSaveHint,
    PanelHint,
    RowBack,
    RowNextPage,
    ConsentGoesTo,
    ConsentAccept,
    ConsentCancel,
    ConsentHostTooLong,
    StatusNoAlertDelivery,
    StatusNoDestination,
    StatusNoDeliveryYet,
    OutcomeDelivered,
    OutcomeRetryPending,
    OutcomeRefused,
    OutcomeGoneDisabled,
    OutcomeRateLimited,
    OutcomeHeld,
    OutcomeNotReady,
    OutcomeNeedsConsent,
    OutcomeNothingWaiting,
    OutcomeTestSending,
}

pub const ALL_MSGS: [Msg; 65] = [
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
    Msg::RowIntegrations,
    Msg::PanelTitle,
    Msg::PanelIntro,
    Msg::PanelWaiting,
    Msg::PanelEmpty,
    Msg::PanelAddHint,
    Msg::PanelSaveHint,
    Msg::PanelHint,
    Msg::RowBack,
    Msg::RowNextPage,
    Msg::ConsentGoesTo,
    Msg::ConsentAccept,
    Msg::ConsentCancel,
    Msg::ConsentHostTooLong,
    Msg::StatusNoAlertDelivery,
    Msg::StatusNoDestination,
    Msg::StatusNoDeliveryYet,
    Msg::OutcomeDelivered,
    Msg::OutcomeRetryPending,
    Msg::OutcomeRefused,
    Msg::OutcomeGoneDisabled,
    Msg::OutcomeRateLimited,
    Msg::OutcomeHeld,
    Msg::OutcomeNotReady,
    Msg::OutcomeNeedsConsent,
    Msg::OutcomeNothingWaiting,
    Msg::OutcomeTestSending,
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
        Msg::RowIntegrations => "Integrations...",
        Msg::PanelTitle => "Integrations",
        Msg::PanelIntro => "Alerts go only to integrations turned on here",
        Msg::PanelWaiting => "Some wait for your consent and send nothing yet",
        Msg::PanelEmpty => "No integrations yet",
        Msg::PanelAddHint => "Add one: stardust integration add KIND ID",
        Msg::PanelSaveHint => "Save a destination: stardust integration url ID URL",
        Msg::PanelHint => "Arrows move. Enter selects. Esc goes back",
        Msg::RowBack => "Back",
        Msg::RowNextPage => "Next page",
        Msg::ConsentGoesTo => "Each alert goes to:",
        Msg::ConsentAccept => "Accept and turn on",
        Msg::ConsentCancel => "Cancel",
        Msg::ConsentHostTooLong => "This host name is too long to show here. Use:",
        Msg::StatusNoAlertDelivery => "No alert delivery",
        Msg::StatusNoDestination => "No destination saved",
        Msg::StatusNoDeliveryYet => "No delivery yet",
        Msg::OutcomeDelivered => "Delivered",
        Msg::OutcomeRetryPending => "Retry pending",
        Msg::OutcomeRefused => "Refused by destination",
        Msg::OutcomeGoneDisabled => "Turned off: destination gone",
        Msg::OutcomeRateLimited => "Rate limited",
        Msg::OutcomeHeld => "Held by quiet hours",
        Msg::OutcomeNotReady => "Not ready",
        Msg::OutcomeNeedsConsent => "Needs consent",
        Msg::OutcomeNothingWaiting => "Nothing waiting",
        Msg::OutcomeTestSending => "Sending test...",
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
