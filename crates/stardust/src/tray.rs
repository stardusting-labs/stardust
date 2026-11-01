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
    menu_text_in, with_connect_status_in, with_connection_note_in, MenuText,
};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Menu text for the current config and check. The ORBIT connection line is
/// there only with auto sign-in on, and "Send test" only with routing on.
pub fn tray_text(config: &Config, live: &LivePet, language: Language) -> MenuText {
    let observed = live.observed();
    let text = with_connection_note_in(
        menu_text_in(config, Utc::now(), language),
        observed,
        language,
    );
    if !config.features().auto_sign_in {
        return text;
    }
    let phase = live.phase();
    let signed_in = match live.signed_in() {
        Some(known) => known,
        // Not known yet and nothing to go on: leave the line out rather
        // than guess.
        None if phase == FlowPhase::Idle && observed == ObservedCheck::Waiting => return text,
        None => observed != ObservedCheck::SignedOut,
    };
    with_connect_status_in(text, config, &phase, observed, signed_in, language)
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
    append_item(&menu, "sign-in", tr(language, Msg::MenuSignIn), true)?;
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
