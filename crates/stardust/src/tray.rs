use crate::icon::tray_rgba;
use crate::inbox::{Inbox, UiMsg};
use stardust_core::schedule::{format_interval, QUICK_PICK_MINUTES};
use stardust_core::text::MenuText;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct Tray {
    icon: TrayIcon,
}

impl Tray {
    pub fn install(inbox: &Inbox, text: &MenuText) -> Result<Self, String> {
        let (rgba, width, height) = tray_rgba();
        let icon = Icon::from_rgba(rgba, width, height).map_err(|err| err.to_string())?;
        let menu = build_menu(text)?;
        let icon = TrayIconBuilder::new()
            .with_tooltip(&text.tooltip)
            .with_icon(icon)
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

    pub fn apply(&self, text: &MenuText) -> Result<(), String> {
        self.icon.set_menu(Some(Box::new(build_menu(text)?)));
        self.icon
            .set_tooltip(Some(text.tooltip.as_str()))
            .map_err(|err| err.to_string())
    }
}

fn build_menu(text: &MenuText) -> Result<Menu, String> {
    let menu = Menu::new();
    append_item(&menu, "next", &text.next_check, false)?;
    append_item(&menu, "last", &text.last_checked, false)?;
    append_item(&menu, "schedule", &text.schedule, false)?;
    append_item(&menu, "integrations", &text.integrations, false)?;
    append_separator(&menu)?;
    append_item(&menu, "character", &text.character, true)?;
    append_item(&menu, "check-now", "Check now", true)?;
    append_separator(&menu)?;
    for minutes in QUICK_PICK_MINUTES {
        append_item(
            &menu,
            &format!("interval-{minutes}"),
            &format!("Every {}", format_interval(minutes)),
            true,
        )?;
    }
    append_separator(&menu)?;
    append_item(&menu, "quit", "Quit", true)?;
    Ok(menu)
}

fn append_item(menu: &Menu, id: &str, label: &str, enabled: bool) -> Result<(), String> {
    let item = MenuItem::with_id(id, label, enabled, None);
    menu.append(&item).map_err(|err| err.to_string())
}

fn append_separator(menu: &Menu) -> Result<(), String> {
    let item = PredefinedMenuItem::separator();
    menu.append(&item).map_err(|err| err.to_string())
}
