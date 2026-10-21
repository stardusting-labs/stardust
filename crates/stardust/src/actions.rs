use chrono::{DateTime, Utc};
use stardust_core::config::{Config, FieldUpdate, ScheduleUpdate};
use stardust_core::schedule::ScheduleError;

pub enum MenuEffect {
    None,
    Quit,
    CheckNow,
    Refresh,
}

pub fn apply_menu(
    config: &mut Config,
    id: &str,
    now: DateTime<Utc>,
) -> Result<MenuEffect, ScheduleError> {
    let minutes = match id {
        "quit" => return Ok(MenuEffect::Quit),
        "check-now" => return Ok(MenuEffect::CheckNow),
        "character" => {
            config.set_character_visible(!config.character_visible());
            return Ok(MenuEffect::Refresh);
        }
        "interval-1" => 1,
        "interval-30" => 30,
        "interval-60" => 60,
        _ => return Ok(MenuEffect::None),
    };
    let update = ScheduleUpdate {
        interval: Some(FieldUpdate::Set(minutes)),
        ..ScheduleUpdate::default()
    };
    config.apply_schedule_update(&update, now)?;
    Ok(MenuEffect::Refresh)
}

#[cfg(test)]
mod tests {
    use super::{apply_menu, MenuEffect};
    use chrono::{TimeZone, Utc};
    use stardust_core::config::Config;

    #[test]
    fn quick_picks_set_the_interval_and_unknown_ids_do_nothing() {
        let mut config = Config::default_for("UTC").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        assert!(matches!(
            apply_menu(&mut config, "interval-30", now).unwrap(),
            MenuEffect::Refresh
        ));
        assert_eq!(config.schedule().interval_minutes(), Some(30));
        assert!(matches!(
            apply_menu(&mut config, "interval-45", now).unwrap(),
            MenuEffect::None
        ));
        assert_eq!(config.schedule().interval_minutes(), Some(30));
        apply_menu(&mut config, "character", now).unwrap();
        assert!(config.character_visible());
    }
}
