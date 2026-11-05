use crate::icon::tray_rgba;
use crate::inbox::{Inbox, UiMsg};
use crate::live::LivePet;
use chrono::Utc;
use stardust_core::config::Config;
use stardust_core::connect::FlowPhase;
use stardust_core::i18n::{every, interval_label, tr, Language, Msg};
use stardust_core::pet::{tray_edge_px, ObservedCheck, PetScale, PetSkin, TrayGlyph};
use stardust_core::schedule::QUICK_PICK_MINUTES;
use stardust_core::text::{
    menu_text_in, with_connect_status_in, with_connection_note_in, MenuText, TrayAccount,
};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Menu text for the current config and check. The ORBIT connection line is
/// there only with auto sign-in or the launch sign-in window on, "Send test"
/// only with routing on, and "Sign out" only with the launch sign-in window on.
pub fn tray_text(config: &Config, live: &LivePet, language: Language) -> MenuText {
    let observed = live.observed();
    let mut text = with_connection_note_in(
        menu_text_in(config, Utc::now(), language),
        observed,
        language,
    );
    let features = config.features();
    if !features.sign_in_on_timer() {
        return text;
    }
    let phase = live.phase();
    let known = if features.launch_sign_in_window {
        let known = live.signed_in_at(Utc::now());
        text.account = tray_account(live.demo(), known);
        known
    } else {
        live.signed_in()
    };
    let signed_in = match known {
        Some(known) => known,
        // Not known yet and nothing to go on: leave the line out rather
        // than guess.
        None if phase == FlowPhase::Idle && observed == ObservedCheck::Waiting => return text,
        None => observed != ObservedCheck::SignedOut,
    };
    with_connect_status_in(text, config, &phase, observed, signed_in, language)
}

fn tray_account(demo: bool, signed_in: Option<bool>) -> TrayAccount {
    match (demo, signed_in) {
        (true, _) => TrayAccount::Hidden,
        (false, Some(true)) => TrayAccount::SignOut,
        (false, Some(false) | None) => TrayAccount::SignIn,
    }
}

pub struct Tray {
    icon: TrayIcon,
}

impl Tray {
    pub fn install(
        inbox: &Inbox,
        text: &MenuText,
        glyph: TrayGlyph,
        scale_factor: f64,
        language: Language,
    ) -> Result<Self, String> {
        let menu = build_menu(text, language)?;
        let icon = TrayIconBuilder::new()
            .with_tooltip(&text.tooltip)
            .with_icon(tray_icon(glyph, scale_factor)?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .build()
            .map_err(|err| err.to_string())?;
        let inbox = inbox.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            inbox.push(UiMsg::Menu(event.id.as_ref().to_string()));
        }));
        Ok(Self { icon })
    }

    pub fn apply(
        &self,
        text: &MenuText,
        glyph: TrayGlyph,
        scale_factor: f64,
        language: Language,
    ) -> Result<(), String> {
        self.icon
            .set_menu(Some(Box::new(build_menu(text, language)?)));
        self.icon
            .set_icon(Some(tray_icon(glyph, scale_factor)?))
            .map_err(|err| err.to_string())?;
        self.icon
            .set_tooltip(Some(text.tooltip.as_str()))
            .map_err(|err| err.to_string())
    }
}

fn tray_icon(glyph: TrayGlyph, scale_factor: f64) -> Result<Icon, String> {
    let (rgba, width, height) = tray_rgba(glyph, tray_edge_px(scale_factor));
    Icon::from_rgba(rgba, width, height).map_err(|err| err.to_string())
}

fn skin_label(skin: PetSkin, language: Language) -> &'static str {
    match skin {
        PetSkin::Astronaut => tr(language, Msg::SkinAstronaut),
        PetSkin::Star => tr(language, Msg::SkinStar),
    }
}

fn scale_label(scale: PetScale, language: Language) -> String {
    format!("{} {}x", tr(language, Msg::RowPetSize), scale.factor())
}

fn build_menu(text: &MenuText, language: Language) -> Result<Menu, String> {
    let menu = Menu::new();
    append_item(&menu, "settings", tr(language, Msg::MenuSettings), true)?;
    match text.account {
        TrayAccount::SignIn => append_item(&menu, "sign-in", tr(language, Msg::MenuSignIn), true)?,
        TrayAccount::SignOut => {
            append_item(&menu, "sign-out", tr(language, Msg::MenuSignOut), true)?;
        }
        TrayAccount::Hidden => {}
    }
    if let Some(status) = &text.status {
        append_item(&menu, "connect-status", status, false)?;
    }
    append_separator(&menu)?;
    append_item(&menu, "next", &text.next_check, false)?;
    append_item(&menu, "last", &text.last_checked, false)?;
    append_item(&menu, "schedule", &text.schedule, false)?;
    append_item(&menu, "integrations", &text.integrations, false)?;
    append_separator(&menu)?;
    append_item(&menu, "character", &text.character, true)?;
    append_item(&menu, "pet", tr(language, Msg::MenuPet), false)?;
    for skin in PetSkin::ALL {
        append_check(
            &menu,
            skin.menu_id(),
            skin_label(skin, language),
            text.pet_skin == skin,
        )?;
    }
    for scale in PetScale::ALL {
        append_check(
            &menu,
            scale.menu_id(),
            &scale_label(scale, language),
            text.pet_scale == scale,
        )?;
    }
    append_item(&menu, "check-now", tr(language, Msg::MenuCheckNow), true)?;
    for (id, label) in &text.tests {
        append_item(&menu, id, label, true)?;
    }
    append_separator(&menu)?;
    for minutes in QUICK_PICK_MINUTES {
        append_item(
            &menu,
            &format!("interval-{minutes}"),
            &every(language, &interval_label(language, minutes)),
            true,
        )?;
    }
    append_separator(&menu)?;
    append_item(&menu, "quit", tr(language, Msg::MenuQuit), true)?;
    Ok(menu)
}

fn append_item(menu: &Menu, id: &str, label: &str, enabled: bool) -> Result<(), String> {
    let item = MenuItem::with_id(id, label, enabled, None);
    menu.append(&item).map_err(|err| err.to_string())
}

fn append_check(menu: &Menu, id: &str, label: &str, checked: bool) -> Result<(), String> {
    let item = CheckMenuItem::with_id(id, label, true, checked, None);
    menu.append(&item).map_err(|err| err.to_string())
}

fn append_separator(menu: &Menu) -> Result<(), String> {
    let item = PredefinedMenuItem::separator();
    menu.append(&item).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::tray_text;
    use crate::live::LivePet;
    use chrono::{TimeDelta, Utc};
    use stardust_core::config::{Config, Feature};
    use stardust_core::i18n::Language;
    use stardust_core::pet::ObservedCheck;
    use stardust_core::text::TrayAccount;

    const ENGLISH: Language = Language::English;

    #[test]
    fn with_every_flag_off_the_tray_always_says_sign_in_and_has_no_status() {
        let config = Config::default_for("UTC").unwrap();
        let live = LivePet::new(1.0);
        for signed_in in [true, false] {
            live.set_signed_in(signed_in);
            let text = tray_text(&config, &live, ENGLISH);
            assert_eq!(text.account, TrayAccount::SignIn);
            assert_eq!(text.status, None);
        }
        live.set_demo(true);
        assert_eq!(
            tray_text(&config, &live, ENGLISH).account,
            TrayAccount::SignIn,
            "--demo keeps its old menu with the flag off"
        );
    }

    #[test]
    fn auto_sign_in_alone_keeps_the_old_account_item() {
        let mut config = Config::default_for("UTC").unwrap();
        config.set_feature(Feature::AutoSignIn, true);
        let live = LivePet::new(1.0);
        live.set_signed_in(true);
        // A lapsed record changes nothing without the new flag.
        live.note_session(Some(Utc::now() - TimeDelta::seconds(5)));
        let text = tray_text(&config, &live, ENGLISH);
        assert_eq!(text.account, TrayAccount::SignIn);
        assert_eq!(text.status.as_deref(), Some("ORBIT: Connected"));
    }

    #[test]
    fn the_flag_shows_the_connection_and_the_matching_account_item() {
        let mut config = Config::default_for("UTC").unwrap();
        config.set_feature(Feature::LaunchSignInWindow, true);
        let live = LivePet::new(1.0);
        let text = tray_text(&config, &live, ENGLISH);
        assert_eq!(text.account, TrayAccount::SignIn, "not known yet");
        live.set_signed_in(false);
        let text = tray_text(&config, &live, ENGLISH);
        assert_eq!(text.account, TrayAccount::SignIn);
        assert_eq!(text.status.as_deref(), Some("ORBIT: Signed out"));
        live.set_signed_in(true);
        let text = tray_text(&config, &live, ENGLISH);
        assert_eq!(text.account, TrayAccount::SignOut);
        assert_eq!(text.status.as_deref(), Some("ORBIT: Connected"));
    }

    #[test]
    fn a_record_that_lapsed_offers_sign_in_again_before_the_next_check() {
        let mut config = Config::default_for("UTC").unwrap();
        config.set_feature(Feature::LaunchSignInWindow, true);
        let live = LivePet::new(1.0);
        live.set_signed_in(true);
        live.set_observed(ObservedCheck::Succeeded {
            item_count: 0,
            high_severity: false,
        });
        live.note_session(Some(Utc::now() + TimeDelta::seconds(120)));
        assert_eq!(
            tray_text(&config, &live, ENGLISH).account,
            TrayAccount::SignOut
        );
        live.note_session(Some(Utc::now() - TimeDelta::seconds(1)));
        let text = tray_text(&config, &live, ENGLISH);
        assert_eq!(text.account, TrayAccount::SignIn);
        assert_eq!(text.status.as_deref(), Some("ORBIT: Signed out"));
    }

    #[test]
    fn demo_mode_offers_no_account_item_under_the_flag() {
        let mut config = Config::default_for("UTC").unwrap();
        config.set_feature(Feature::LaunchSignInWindow, true);
        let live = LivePet::new(1.0);
        live.set_demo(true);
        live.set_signed_in(true);
        assert_eq!(
            tray_text(&config, &live, ENGLISH).account,
            TrayAccount::Hidden
        );
    }
}
