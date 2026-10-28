use stardust_core::config::{FieldUpdate, ScheduleUpdate};
use stardust_core::schedule::parse_interval;
use std::path::PathBuf;

pub enum Command {
    Help,
    Run {
        config: Option<PathBuf>,
        headless: bool,
    },
    CheckOnce {
        config: Option<PathBuf>,
    },
    ScheduleShow {
        config: Option<PathBuf>,
    },
    ScheduleSet {
        config: Option<PathBuf>,
        update: ScheduleUpdate,
    },
    Character {
        config: Option<PathBuf>,
        action: CharacterAction,
    },
}

pub enum CharacterAction {
    Show,
    Hide,
    Position(i32, i32),
}

pub fn usage() -> String {
    "\
Stardust shows ORBIT check results on a schedule you set.

Usage:
  stardust [--config PATH] [--headless]
  stardust [--config PATH] --check-once
  stardust [--config PATH] schedule show
  stardust [--config PATH] schedule set [options]
  stardust [--config PATH] character show|hide
  stardust [--config PATH] character position X Y

Schedule options:
  --interval DURATION    1 minute through 24 hours, such as 45m or 2h
  --time HH:MM           clock time; repeat the flag for more than one
  --weekdays DAYS        mon-fri, sat,sun, or all
  --quiet HH:MM-HH:MM    high-severity items are the only ones shown in this span
  --timezone NAME        IANA timezone
  --clear-interval
  --clear-times
  --clear-quiet

The tray lists 1, 30, and 60 minute suggestions. A typed interval is set with schedule set.
Checks run at the next interval step or clock time, whichever comes first.
A high-severity item is shown from the check that observed it.
"
    .to_string()
}

pub fn parse(args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Command, String> {
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Command::Help);
    }
    let mut index = 0;
    let mut config = None;
    let mut headless = false;
    let mut check_once = false;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--config" {
            index += 1;
            let path = args
                .get(index)
                .ok_or_else(|| "missing path after --config".to_string())?;
            config = Some(PathBuf::from(path));
            index += 1;
            continue;
        }
        if arg == "--headless" {
            headless = true;
            index += 1;
            continue;
        }
        if arg == "--check-once" {
            check_once = true;
            index += 1;
            continue;
        }
        break;
    }
    if index >= args.len() {
        if check_once && headless {
            return Err("choose --check-once or --headless".to_string());
        }
        if check_once {
            return Ok(Command::CheckOnce { config });
        }
        return Ok(Command::Run { config, headless });
    }
    if headless || check_once {
        return Err(
            "flags after a command are not used; pass --config, --headless, or --check-once first"
                .to_string(),
        );
    }
    match args[index].as_str() {
        "schedule" => parse_schedule(&args, index + 1, config),
        "character" => parse_character(&args, index + 1, config),
        other => Err(format!("unknown command {other}")),
    }
}

fn parse_schedule(
    args: &[String],
    mut index: usize,
    config: Option<PathBuf>,
) -> Result<Command, String> {
    let sub = args
        .get(index)
        .ok_or_else(|| "missing schedule command".to_string())?;
    index += 1;
    match sub.as_str() {
        "show" => {
            if index != args.len() {
                return Err("schedule show takes no further arguments".to_string());
            }
            Ok(Command::ScheduleShow { config })
        }
        "set" => {
            let update = parse_schedule_set(&args[index..])?;
            Ok(Command::ScheduleSet { config, update })
        }
        other => Err(format!("unknown schedule command {other}")),
    }
}

fn parse_schedule_set(args: &[String]) -> Result<ScheduleUpdate, String> {
    let mut update = ScheduleUpdate {
        interval: None,
        clock_times: None,
        weekdays: None,
        quiet_hours: None,
        timezone: None,
    };
    let mut times = Vec::new();
    let mut saw_time = false;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = |index: usize, flag: &str| -> Result<String, String> {
            args.get(index + 1)
                .filter(|item| !item.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("missing value after {flag}"))
        };
        match flag {
            "--interval" => {
                if update.interval.is_some() {
                    return Err("repeat --interval once".to_string());
                }
                let text = value(index, flag)?;
                update.interval = Some(FieldUpdate::Set(
                    parse_interval(&text).map_err(|err| err.to_string())?,
                ));
                index += 2;
            }
            "--clear-interval" => {
                if update.interval.is_some() {
                    return Err("--interval and --clear-interval cannot be combined".to_string());
                }
                update.interval = Some(FieldUpdate::Clear);
                index += 1;
            }
            "--time" => {
                saw_time = true;
                times.push(value(index, flag)?);
                index += 2;
            }
            "--clear-times" => {
                if saw_time {
                    return Err("--time and --clear-times cannot be combined".to_string());
                }
                update.clock_times = Some(Vec::new());
                index += 1;
            }
            "--weekdays" => {
                if update.weekdays.is_some() {
                    return Err("repeat --weekdays once".to_string());
                }
                update.weekdays = Some(value(index, flag)?);
                index += 2;
            }
            "--quiet" => {
                if update.quiet_hours.is_some() {
                    return Err("--quiet and --clear-quiet cannot be combined".to_string());
                }
                update.quiet_hours = Some(FieldUpdate::Set(value(index, flag)?));
                index += 2;
            }
            "--clear-quiet" => {
                if update.quiet_hours.is_some() {
                    return Err("--quiet and --clear-quiet cannot be combined".to_string());
                }
                update.quiet_hours = Some(FieldUpdate::Clear);
                index += 1;
            }
            "--timezone" => {
                if update.timezone.is_some() {
                    return Err("repeat --timezone once".to_string());
                }
                update.timezone = Some(value(index, flag)?);
                index += 2;
            }
            other => return Err(format!("unknown schedule option {other}")),
        }
    }
    if saw_time {
        if update.clock_times.is_some() {
            return Err("--time and --clear-times cannot be combined".to_string());
        }
        update.clock_times = Some(times);
    }
    if update.is_empty() {
        return Err("schedule set needs at least one option".to_string());
    }
    Ok(update)
}

fn parse_character(
    args: &[String],
    mut index: usize,
    config: Option<PathBuf>,
) -> Result<Command, String> {
    let sub = args
        .get(index)
        .ok_or_else(|| "missing character command".to_string())?;
    index += 1;
    let action = match sub.as_str() {
        "show" => CharacterAction::Show,
        "hide" => CharacterAction::Hide,
        "position" => {
            let x = parse_coord(args.get(index), "x")?;
            let y = parse_coord(args.get(index + 1), "y")?;
            index += 2;
            CharacterAction::Position(x, y)
        }
        other => return Err(format!("unknown character command {other}")),
    };
    if index != args.len() {
        return Err("unexpected arguments after the character command".to_string());
    }
    Ok(Command::Character { config, action })
}

fn parse_coord(value: Option<&String>, name: &str) -> Result<i32, String> {
    let text = value.ok_or_else(|| format!("missing {name} position"))?;
    if text.starts_with('-')
        && text
            .chars()
            .nth(1)
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        return Err(format!("{text} is not a position"));
    }
    text.parse::<i32>()
        .map_err(|_| format!("could not read position {text}"))
}

#[cfg(test)]
mod tests {
    use super::{parse, usage, CharacterAction, Command};
    use chrono::{TimeZone, Utc};
    use stardust_core::config::Config;
    use stardust_core::copy::find_disallowed_copy;

    #[test]
    fn typed_schedule_applies_and_usage_stays_plain() {
        let command = parse([
            "--config",
            "/tmp/stardust.json",
            "schedule",
            "set",
            "--interval",
            "45m",
            "--time",
            "09:00",
            "--time",
            "13:00",
            "--time",
            "18:00",
            "--weekdays",
            "mon-fri",
            "--quiet",
            "22:00-07:00",
            "--timezone",
            "America/New_York",
        ])
        .unwrap();
        let Command::ScheduleSet { config, update } = command else {
            panic!("expected schedule set");
        };
        assert_eq!(config.unwrap().to_string_lossy(), "/tmp/stardust.json");
        let mut loaded = Config::default_for("UTC").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap();
        loaded.apply_schedule_update(&update, now).unwrap();
        assert_eq!(loaded.schedule().interval_minutes(), Some(45));
        assert_eq!(loaded.schedule().clock_times().len(), 3);
        assert!(loaded.schedule().quiet_hours().is_some());
        assert_eq!(loaded.schedule().timezone().name(), "America/New_York");
        let help = usage();
        assert_eq!(find_disallowed_copy(&help), None);
        assert!(!help.to_ascii_lowercase().contains("chat"));
    }

    #[test]
    fn negative_coordinates_are_positions() {
        let command = parse(["character", "position", "-20", "40"]).unwrap();
        let Command::Character {
            action: CharacterAction::Position(x, y),
            ..
        } = command
        else {
            panic!("expected position");
        };
        assert_eq!((x, y), (-20, 40));
        assert!(parse(["schedule", "set", "--interval", "45m", "--clear-interval"]).is_err());
        assert!(parse(["schedule", "set", "--time", "09:00", "--clear-times"]).is_err());
        assert!(parse(["schedule", "set", "--quiet", "22:00-07:00", "--clear-quiet"]).is_err());
    }
}
