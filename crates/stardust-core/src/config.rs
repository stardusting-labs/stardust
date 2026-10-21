//! Local config: character position, schedule, and check cursor.
//!
//! Tokens are not stored here.

use crate::position::ScreenPoint;
use crate::schedule::{
    day_token, parse_clock, parse_interval, parse_quiet, parse_timezone, parse_weekdays, Schedule,
    ScheduleError, Weekdays,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CheckState {
    #[serde(default)]
    pub interval_anchor: Option<DateTime<Utc>>,
    #[serde(default)]
    pub alerts_cursor: Option<String>,
    #[serde(default)]
    pub last_checked: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldUpdate<T> {
    Set(T),
    Clear,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ScheduleUpdate {
    pub interval: Option<FieldUpdate<u32>>,
    pub clock_times: Option<Vec<String>>,
    pub weekdays: Option<String>,
    pub quiet_hours: Option<FieldUpdate<String>>,
    pub timezone: Option<String>,
}

impl ScheduleUpdate {
    pub fn is_empty(&self) -> bool {
        self.interval.is_none()
            && self.clock_times.is_none()
            && self.weekdays.is_none()
            && self.quiet_hours.is_none()
            && self.timezone.is_none()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read config: {0}")]
    Io(#[from] io::Error),
    #[error("could not parse config: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleFile {
    interval_minutes: Option<u32>,
    clock_times: Vec<String>,
    weekdays: Vec<String>,
    quiet_hours: Option<String>,
    timezone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    character_visible: bool,
    character_position: ScreenPoint,
    schedule: ScheduleFile,
    #[serde(default)]
    state: CheckState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    file: ConfigFile,
    schedule: Schedule,
}

impl Config {
    pub fn default_for(timezone: &str) -> Result<Self, ScheduleError> {
        let file = ConfigFile {
            character_visible: false,
            character_position: ScreenPoint { x: 64, y: 64 },
            schedule: ScheduleFile {
                interval_minutes: Some(60),
                clock_times: Vec::new(),
                weekdays: Weekdays::ALL
                    .days()
                    .map(|day| day_token(day).to_string())
                    .collect(),
                quiet_hours: None,
                timezone: timezone.to_string(),
            },
            state: CheckState::default(),
        };
        Self::from_file(file)
    }

    pub fn character_visible(&self) -> bool {
        self.file.character_visible
    }

    pub fn character_position(&self) -> ScreenPoint {
        self.file.character_position
    }

    pub fn set_character_visible(&mut self, visible: bool) {
        self.file.character_visible = visible;
    }

    pub fn set_character_position(&mut self, position: ScreenPoint) {
        self.file.character_position = position;
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    pub fn state(&self) -> &CheckState {
        &self.file.state
    }

    pub fn state_mut(&mut self) -> &mut CheckState {
        &mut self.file.state
    }

    pub fn apply_schedule_update(
        &mut self,
        update: &ScheduleUpdate,
        now: DateTime<Utc>,
    ) -> Result<(), ScheduleError> {
        let mut file = self.file.schedule.clone();
        let interval_changed = update.interval.is_some();
        if let Some(interval) = &update.interval {
            file.interval_minutes = match interval {
                FieldUpdate::Set(minutes) => Some(*minutes),
                FieldUpdate::Clear => None,
            };
        }
        if let Some(times) = &update.clock_times {
            file.clock_times = times.clone();
        }
        if let Some(days) = &update.weekdays {
            let parsed = parse_weekdays(days)?;
            file.weekdays = parsed
                .days()
                .map(|day| day_token(day).to_string())
                .collect();
        }
        if let Some(quiet) = &update.quiet_hours {
            file.quiet_hours = match quiet {
                FieldUpdate::Set(text) => Some(parse_quiet(text)?.label()),
                FieldUpdate::Clear => None,
            };
        }
        if let Some(timezone) = &update.timezone {
            file.timezone = timezone.clone();
        }
        let schedule = schedule_from_file(&file)?;
        self.file.schedule = schedule_file(&schedule);
        self.schedule = schedule;
        if interval_changed {
            self.file.state.interval_anchor = Some(now);
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(&self.file)?)
    }

    fn from_file(file: ConfigFile) -> Result<Self, ScheduleError> {
        let schedule = schedule_from_file(&file.schedule)?;
        let mut file = file;
        file.schedule = schedule_file(&schedule);
        Ok(Self { file, schedule })
    }
}

fn schedule_from_file(file: &ScheduleFile) -> Result<Schedule, ScheduleError> {
    if let Some(minutes) = file.interval_minutes {
        let _ = parse_interval(&minutes.to_string())?;
    }
    let mut clock_times = Vec::with_capacity(file.clock_times.len());
    for time in &file.clock_times {
        clock_times.push(parse_clock(time)?);
    }
    let weekdays = if file.weekdays.is_empty() {
        return Err(ScheduleError::Weekdays);
    } else {
        parse_weekdays(&file.weekdays.join(","))?
    };
    let quiet = file.quiet_hours.as_deref().map(parse_quiet).transpose()?;
    let timezone = parse_timezone(&file.timezone)?;
    Schedule::try_new(
        file.interval_minutes,
        clock_times,
        weekdays,
        quiet,
        timezone,
    )
}

fn schedule_file(schedule: &Schedule) -> ScheduleFile {
    ScheduleFile {
        interval_minutes: schedule.interval_minutes(),
        clock_times: schedule
            .clock_times()
            .iter()
            .map(|time| crate::schedule::format_time(*time))
            .collect(),
        weekdays: schedule
            .weekdays()
            .days()
            .map(|day| day_token(day).to_string())
            .collect(),
        quiet_hours: schedule.quiet_hours().map(|quiet| quiet.label()),
        timezone: schedule.timezone().name().to_string(),
    }
}

pub fn parse_config(text: &str) -> Result<Config, ConfigError> {
    let file: ConfigFile = serde_json::from_str(text)?;
    Ok(Config::from_file(file)?)
}

pub fn load_config(path: &Path) -> Result<Option<Config>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(parse_config(&text)?)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn save_config(path: &Path, config: &Config) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&config.file)?;
    bytes.push(b'\n');
    let tmp = temp_path(path);
    fs::write(&tmp, &bytes)?;
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

pub fn config_path_from(
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    app_data: Option<&str>,
) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let _ = (xdg_config_home, home);
        return PathBuf::from(app_data.unwrap_or("."))
            .join("Stardust")
            .join("config.json");
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (xdg_config_home, app_data);
        return PathBuf::from(home.unwrap_or("."))
            .join("Library")
            .join("Application Support")
            .join("Stardust")
            .join("config.json");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = app_data;
        if let Some(xdg) = xdg_config_home.filter(|value| !value.is_empty()) {
            return PathBuf::from(xdg).join("stardust").join("config.json");
        }
        PathBuf::from(home.unwrap_or("."))
            .join(".config")
            .join("stardust")
            .join("config.json")
    }
}

pub fn default_config_path() -> PathBuf {
    config_path_from(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        config_path_from, load_config, parse_config, save_config, Config, FieldUpdate,
        ScheduleUpdate,
    };
    use crate::position::{layout_bubble, LayoutRequest, ScreenPoint, ScreenRect, WindowSize};
    use chrono::{TimeZone, Utc};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn position_and_visibility_round_trip_without_rewriting_an_offscreen_point() {
        let mut config = Config::default_for("UTC").unwrap();
        let stored = ScreenPoint { x: -1920, y: 240 };
        config.set_character_position(stored);
        config.set_character_visible(true);
        config.set_character_visible(false);
        assert_eq!(config.character_position(), stored);
        assert!(!config.character_visible());

        let text = config.to_json().unwrap();
        let loaded = parse_config(&text).unwrap();
        assert_eq!(loaded.character_position(), stored);
        assert!(!loaded.character_visible());

        let monitors = [ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        let layout = layout_bubble(&LayoutRequest {
            character_visible: loaded.character_visible(),
            stored_position: loaded.character_position(),
            monitors: &monitors,
            character_size: WindowSize {
                width: 80,
                height: 80,
            },
            bubble_size: WindowSize {
                width: 200,
                height: 60,
            },
        });
        assert!(!layout.show_character);
        assert_eq!(loaded.character_position(), stored);
        assert!(layout.origin.x >= 0);
    }

    #[test]
    fn custom_schedule_persists_and_interval_changes_reset_the_anchor() {
        let mut config = Config::default_for("America/New_York").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        let update = ScheduleUpdate {
            interval: Some(FieldUpdate::Set(45)),
            clock_times: Some(vec![
                "9:00".to_string(),
                "13:00".to_string(),
                "18:00".to_string(),
            ]),
            weekdays: Some("mon-fri".to_string()),
            quiet_hours: Some(FieldUpdate::Set("22:00-07:00".to_string())),
            ..ScheduleUpdate::default()
        };
        config.apply_schedule_update(&update, now).unwrap();
        assert_eq!(config.schedule().interval_minutes(), Some(45));
        assert_eq!(config.schedule().clock_times().len(), 3);
        assert_eq!(config.state().interval_anchor, Some(now));
        assert!(config
            .schedule()
            .describe()
            .contains("quiet hours 22:00-07:00"));

        let quiet_only = ScheduleUpdate {
            quiet_hours: Some(FieldUpdate::Clear),
            ..ScheduleUpdate::default()
        };
        config
            .apply_schedule_update(&quiet_only, now + chrono::TimeDelta::minutes(5))
            .unwrap();
        assert_eq!(config.state().interval_anchor, Some(now));
        assert!(config.schedule().quiet_hours().is_none());

        let dir = std::env::temp_dir().join(format!(
            "stardust-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        save_config(&path, &config).unwrap();
        let loaded = load_config(&path).unwrap().unwrap();
        assert_eq!(loaded.schedule(), config.schedule());
        assert_eq!(loaded.character_position(), config.character_position());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_json_fields_are_rejected_and_paths_follow_the_home_layout() {
        let text = r#"{"character_visible":false,"character_position":{"x":1,"y":2},"schedule":{"interval_minutes":60,"clock_times":[],"weekdays":["mon"],"timezone":"UTC"},"token":"no"}"#;
        assert!(parse_config(text).is_err());
        let minimal = r#"{"character_visible":false,"character_position":{"x":1,"y":2},"schedule":{"interval_minutes":60,"clock_times":[],"weekdays":["mon","tue","wed","thu","fri","sat","sun"],"timezone":"UTC"}}"#;
        let config = parse_config(minimal).unwrap();
        assert!(config.state().last_checked.is_none());
        assert_eq!(
            config_path_from(Some("/tmp/xdg"), Some("/home/person"), None),
            std::path::PathBuf::from("/tmp/xdg/stardust/config.json")
        );
        assert_eq!(
            config_path_from(None, Some("/home/person"), None),
            std::path::PathBuf::from("/home/person/.config/stardust/config.json")
        );
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
}
