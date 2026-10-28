use chrono::{DateTime, Utc};
use stardust_core::config::{Config, FieldUpdate, ScheduleUpdate};
use stardust_core::pet::{scale_from_menu, skin_from_menu};
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
    match id {
        "quit" => Ok(MenuEffect::Quit),
        "check-now" => Ok(MenuEffect::CheckNow),
        "character" => {
            config.set_character_visible(!config.character_visible());
            Ok(MenuEffect::Refresh)
        }
        "interval-1" => apply_interval(config, 1, now),
        "interval-30" => apply_interval(config, 30, now),
        "interval-60" => apply_interval(config, 60, now),
        other => {
            if let Some(skin) = skin_from_menu(other) {
                config.set_pet_skin(skin);
                return Ok(MenuEffect::Refresh);
            }
            if let Some(scale) = scale_from_menu(other) {
                config.set_pet_scale(scale);
                return Ok(MenuEffect::Refresh);
            }
            Ok(MenuEffect::None)
        }
    }
}

fn apply_interval(
    config: &mut Config,
    minutes: u32,
    now: DateTime<Utc>,
) -> Result<MenuEffect, ScheduleError> {
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
    use stardust_core::pet::{PetScale, PetSkin};

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

    #[test]
    fn tray_skin_and_scale_apply_without_a_restart() {
        let mut config = Config::default_for("UTC").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        assert_eq!(config.pet_skin(), PetSkin::Astronaut);
        assert_eq!(config.pet_scale(), PetScale::X2);
        assert!(matches!(
            apply_menu(&mut config, PetSkin::Star.menu_id(), now).unwrap(),
            MenuEffect::Refresh
        ));
        assert_eq!(config.pet_skin(), PetSkin::Star);
        assert!(matches!(
            apply_menu(&mut config, PetScale::X3.menu_id(), now).unwrap(),
            MenuEffect::Refresh
        ));
        assert_eq!(config.pet_scale(), PetScale::X3);
        apply_menu(&mut config, PetSkin::Astronaut.menu_id(), now).unwrap();
        apply_menu(&mut config, PetScale::X2.menu_id(), now).unwrap();
        assert_eq!(config.pet_skin(), PetSkin::Astronaut);
        assert_eq!(config.pet_scale(), PetScale::X2);
        assert_eq!(config.schedule().interval_minutes(), Some(60));
    }
}
