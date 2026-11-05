use stardust_core::api::Severity;
use stardust_core::config::{Feature, FieldUpdate, RoutingUpdate, ScheduleUpdate};
use stardust_core::schedule::parse_interval;
use std::path::PathBuf;

pub enum IntegrationAction {
    List,
    Add {
        kind: String,
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Url {
        id: String,
        url: String,
        signing_secret: Option<String>,
    },
    Token {
        id: String,
        token: String,
    },
    Oauth {
        id: String,
        client_id: String,
    },
    Connect {
        id: String,
    },
    /// PagerDuty Events API v2 routing key, kept in the keychain.
    Key {
        id: String,
        key: String,
    },
    /// Per-sink routing rules: minimum severity, hourly limit, quiet-hours
    /// bypass for high-severity alerts.
    Route {
        id: String,
        update: RoutingUpdate,
    },
    /// Post the fixed test message now.
    Test {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureAction {
    List,
    Set { feature: Feature, on: bool },
}

pub enum Command {
    Help,
    Run {
        config: Option<PathBuf>,
        headless: bool,
        demo: bool,
    },
    CheckOnce {
        config: Option<PathBuf>,
        demo: bool,
    },
    SignIn {
        config: Option<PathBuf>,
    },
    SignOut {
        config: Option<PathBuf>,
        demo: bool,
    },
    Feature {
        config: Option<PathBuf>,
        action: FeatureAction,
    },
    Autostart(AutostartAction),
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
    Integration {
        config: Option<PathBuf>,
        action: IntegrationAction,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutostartAction {
    On,
    Off,
    Status,
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
  stardust [--config PATH] [--headless] [--demo]
  stardust [--config PATH] [--demo] --check-once
  stardust [--config PATH] sign-in
  stardust [--config PATH] sign-out
  stardust autostart on|off|status
  stardust [--config PATH] schedule show
  stardust [--config PATH] schedule set [options]
  stardust [--config PATH] character show|hide
  stardust [--config PATH] character position X Y
  stardust [--config PATH] integration list
  stardust [--config PATH] integration add KIND ID
  stardust [--config PATH] integration enable ID
  stardust [--config PATH] integration disable ID
  stardust [--config PATH] integration url ID URL [SIGNING_SECRET]
  stardust [--config PATH] integration token ID TOKEN
  stardust [--config PATH] integration oauth ID --client-id CLIENT_ID
  stardust [--config PATH] integration connect ID
  stardust [--config PATH] integration key ID ROUTING_KEY
  stardust [--config PATH] integration route ID [route options]
  stardust [--config PATH] integration test ID
  stardust [--config PATH] feature list
  stardust [--config PATH] feature NAME on|off

Schedule options:
  --interval DURATION    1 minute through 24 hours, such as 45m or 2h
  --time HH:MM           clock time; repeat the flag for more than one
  --weekdays DAYS        mon-fri, sat,sun, or all
  --quiet HH:MM-HH:MM    high-severity items are the only ones shown in this span
  --timezone NAME        IANA timezone
  --clear-interval
  --clear-times
  --clear-quiet

Route options:
  --min-severity LEVEL   low, medium, high, or default (high for PagerDuty, low for the rest)
  --max-per-hour N       1 through 3600, or default (30)
  --quiet-bypass on|off  high-severity alerts still go out during quiet hours, or default (on)

The tray lists 1, 30, and 60 minute suggestions. A typed interval is set with schedule set.
Checks run at the next interval step or clock time, whichever comes first.
A high-severity item is shown from the check that observed it.
The tray pet is an astronaut or a star. Choose the skin and a 2x or 3x size from the tray menu.
sign-in stores an ORBIT access token in the system keychain. The default server is https://helloorbit.app.
Set server_base_url in the config file, or STARDUST_SERVER_URL, to use another server.
An expired access token is renewed in the background when ORBIT sent a refresh token.
Settings and sign-in are also in the tray menu. The first start opens the settings window.
autostart on starts Stardust when you log in. It is off until you turn it on.
--demo uses sample items and does not contact ORBIT.

KIND is slack, teams, json, discord, pagerduty, orbit-mcp, or slack-mcp.
Every integration stays off until enable. A destination URL, signing secret, routing key, and access token stay in the system keychain.
Pass - as URL, SIGNING_SECRET, TOKEN, or ROUTING_KEY to read it from standard input, which keeps it out of the process list and shell history.

NAME is auto-sign-in, jittered-backoff, routing, or launch-sign-in-window. Each feature is off until turned on. Quit Stardust before changing one.
auto-sign-in opens the ORBIT sign-in page from the check timer on the first start and shows the connection in the tray.
launch-sign-in-window opens a sign-in window when Stardust starts with no usable sign-in, and adds the connection and Sign out to the tray.
jittered-backoff waits a random time, up to a limit that doubles and stops at 15 minutes, after a failed check.
routing sends one message per new alert to each enabled slack, teams, discord, and pagerduty integration,
using its minimum severity, hourly limit, and quiet hours, and adds a Send test item for each to the tray.
With routing on, teams takes a Workflows webhook URL. pagerduty takes the Events API v2 URL, or none for the US one, and a routing key.
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
    let mut demo = false;
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
        if arg == "--demo" {
            demo = true;
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
            return Ok(Command::CheckOnce { config, demo });
        }
        return Ok(Command::Run {
            config,
            headless,
            demo,
        });
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
        "integration" => parse_integration(&args, index + 1, config),
        "feature" => parse_feature(&args, index + 1, config),
        "sign-in" => {
            if demo {
                return Err("demo mode does not sign in".to_string());
            }
            if index + 1 != args.len() {
                return Err("sign-in takes no further arguments".to_string());
            }
            Ok(Command::SignIn { config })
        }
        "sign-out" => {
            if index + 1 != args.len() {
                return Err("sign-out takes no further arguments".to_string());
            }
            Ok(Command::SignOut { config, demo })
        }
        "autostart" => {
            let action = match args.get(index + 1).map(String::as_str) {
                Some("on") => AutostartAction::On,
                Some("off") => AutostartAction::Off,
                Some("status") => AutostartAction::Status,
                _ => return Err("autostart takes on, off, or status".to_string()),
            };
            if index + 2 != args.len() {
                return Err("autostart takes one argument".to_string());
            }
            Ok(Command::Autostart(action))
        }
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

fn parse_integration(
    args: &[String],
    mut index: usize,
    config: Option<PathBuf>,
) -> Result<Command, String> {
    let sub = args
        .get(index)
        .ok_or_else(|| "missing integration command".to_string())?;
    index += 1;
    let action = match sub.as_str() {
        "list" => {
            if index != args.len() {
                return Err("integration list takes no further arguments".to_string());
            }
            IntegrationAction::List
        }
        "add" => {
            let kind = required(args, index, "kind")?;
            let id = required(args, index + 1, "id")?;
            index += 2;
            finish(index, args.len())?;
            IntegrationAction::Add { kind, id }
        }
        "enable" => IntegrationAction::Enable {
            id: one_id(args, index)?,
        },
        "disable" => IntegrationAction::Disable {
            id: one_id(args, index)?,
        },
        "url" => {
            let id = required(args, index, "id")?;
            let url = required(args, index + 1, "url")?;
            index += 2;
            let signing_secret = if index < args.len() {
                let secret = required(args, index, "signing secret")?;
                index += 1;
                Some(secret)
            } else {
                None
            };
            finish(index, args.len())?;
            IntegrationAction::Url {
                id,
                url,
                signing_secret,
            }
        }
        "token" => {
            let id = required(args, index, "id")?;
            let token = required(args, index + 1, "token")?;
            index += 2;
            finish(index, args.len())?;
            IntegrationAction::Token { id, token }
        }
        "oauth" => {
            let id = required(args, index, "id")?;
            index += 1;
            if args.get(index).map(String::as_str) != Some("--client-id") {
                return Err("oauth needs --client-id".to_string());
            }
            let client_id = required(args, index + 1, "client id")?;
            index += 2;
            finish(index, args.len())?;
            IntegrationAction::Oauth { id, client_id }
        }
        "connect" => IntegrationAction::Connect {
            id: one_id(args, index)?,
        },
        "key" => {
            let id = required(args, index, "id")?;
            let key = required(args, index + 1, "routing key")?;
            index += 2;
            finish(index, args.len())?;
            IntegrationAction::Key { id, key }
        }
        "route" => {
            let id = required(args, index, "id")?;
            let update = parse_route(&args[index + 1..])?;
            IntegrationAction::Route { id, update }
        }
        "test" => IntegrationAction::Test {
            id: one_id(args, index)?,
        },
        other => return Err(format!("unknown integration command {other}")),
    };
    Ok(Command::Integration { config, action })
}

fn parse_route(args: &[String]) -> Result<RoutingUpdate, String> {
    let mut update = RoutingUpdate::default();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if !matches!(flag, "--min-severity" | "--max-per-hour" | "--quiet-bypass") {
            return Err(format!("unknown route option {flag}"));
        }
        let value = args
            .get(index + 1)
            .filter(|item| !item.starts_with("--"))
            .map(String::as_str)
            .ok_or_else(|| format!("missing value after {flag}"))?;
        match flag {
            "--min-severity" => {
                if update.min_severity.is_some() {
                    return Err("repeat --min-severity once".to_string());
                }
                update.min_severity = Some(match value {
                    "default" => FieldUpdate::Clear,
                    "low" => FieldUpdate::Set(Severity::Low),
                    "medium" => FieldUpdate::Set(Severity::Medium),
                    "high" => FieldUpdate::Set(Severity::High),
                    _ => {
                        return Err("--min-severity takes low, medium, high, or default".to_string())
                    }
                });
            }
            "--max-per-hour" => {
                if update.max_per_hour.is_some() {
                    return Err("repeat --max-per-hour once".to_string());
                }
                update.max_per_hour = Some(match value {
                    "default" => FieldUpdate::Clear,
                    number => FieldUpdate::Set(number.parse::<u32>().map_err(|_| {
                        "--max-per-hour takes 1 through 3600, or default".to_string()
                    })?),
                });
            }
            _ => {
                if update.quiet_high_bypass.is_some() {
                    return Err("repeat --quiet-bypass once".to_string());
                }
                update.quiet_high_bypass = Some(match value {
                    "default" => FieldUpdate::Clear,
                    "on" => FieldUpdate::Set(true),
                    "off" => FieldUpdate::Set(false),
                    _ => return Err("--quiet-bypass takes on, off, or default".to_string()),
                });
            }
        }
        index += 2;
    }
    if update == RoutingUpdate::default() {
        return Err("integration route needs at least one option".to_string());
    }
    Ok(update)
}

fn parse_feature(
    args: &[String],
    index: usize,
    config: Option<PathBuf>,
) -> Result<Command, String> {
    let rest = &args[index.min(args.len())..];
    let action = match rest {
        [list] if list == "list" => FeatureAction::List,
        [name, state] => {
            let feature = Feature::parse(name).ok_or_else(|| {
                format!(
                    "unknown feature {name}; use auto-sign-in, jittered-backoff, routing, or launch-sign-in-window"
                )
            })?;
            let on = match state.as_str() {
                "on" => true,
                "off" => false,
                _ => return Err("feature takes on or off".to_string()),
            };
            FeatureAction::Set { feature, on }
        }
        _ => return Err("feature takes list, or a name and on or off".to_string()),
    };
    Ok(Command::Feature { config, action })
}

fn required(args: &[String], index: usize, name: &str) -> Result<String, String> {
    args.get(index)
        .filter(|item| !item.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("missing {name}"))
}

fn one_id(args: &[String], index: usize) -> Result<String, String> {
    let id = required(args, index, "id")?;
    finish(index + 1, args.len())?;
    Ok(id)
}

fn finish(index: usize, len: usize) -> Result<(), String> {
    if index != len {
        return Err("unexpected arguments after the integration command".to_string());
    }
    Ok(())
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
    use super::{parse, usage, AutostartAction, CharacterAction, Command};
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

    #[test]
    fn integration_commands_parse_without_printing_a_secret() {
        let command = parse(["integration", "add", "json", "ops"]).unwrap();
        let Command::Integration {
            action: super::IntegrationAction::Add { kind, id },
            ..
        } = command
        else {
            panic!("expected add");
        };
        assert_eq!((kind, id), ("json".to_string(), "ops".to_string()));
        assert!(parse(["integration", "enable", "ops"]).is_ok());
        assert!(parse(["integration", "oauth", "slack", "--client-id", "123.456"]).is_ok());
        assert!(parse(["integration", "url"]).is_err());
        assert!(parse(["sign-in"]).is_ok());
        assert!(matches!(
            parse(["autostart", "on"]).unwrap(),
            Command::Autostart(AutostartAction::On)
        ));
        assert!(matches!(
            parse(["autostart", "status"]).unwrap(),
            Command::Autostart(AutostartAction::Status)
        ));
        assert!(parse(["autostart"]).is_err());
        assert!(parse(["autostart", "on", "now"]).is_err());
        assert!(parse(["language", "en"]).is_err());
        assert_eq!(find_disallowed_copy(&usage()), None);
        assert!(parse(["--demo", "sign-in"]).is_err());
        assert!(parse(["--demo", "--check-once"]).is_ok());
        assert!(!usage().to_ascii_lowercase().contains("chat"));
    }

    #[test]
    fn features_are_listed_and_switched_by_name() {
        use super::FeatureAction;
        use stardust_core::config::Feature;
        assert!(matches!(
            parse(["--config", "/tmp/s.json", "feature", "list"]).unwrap(),
            Command::Feature {
                config: Some(_),
                action: FeatureAction::List
            }
        ));
        assert!(matches!(
            parse(["feature", "auto-sign-in", "on"]).unwrap(),
            Command::Feature {
                action: FeatureAction::Set {
                    feature: Feature::AutoSignIn,
                    on: true
                },
                ..
            }
        ));
        assert!(matches!(
            parse(["feature", "routing", "off"]).unwrap(),
            Command::Feature {
                action: FeatureAction::Set {
                    feature: Feature::Routing,
                    on: false
                },
                ..
            }
        ));
        assert!(matches!(
            parse(["feature", "launch-sign-in-window", "on"]).unwrap(),
            Command::Feature {
                action: FeatureAction::Set {
                    feature: Feature::LaunchSignInWindow,
                    on: true
                },
                ..
            }
        ));
        assert!(parse(["feature", "nope", "on"])
            .err()
            .unwrap()
            .contains("launch-sign-in-window"));
        assert!(super::usage().contains("launch-sign-in-window opens a sign-in window"));
        for refused in [
            &["feature"][..],
            &["feature", "list", "now"],
            &["feature", "telemetry", "on"],
            &["feature", "routing", "yes"],
            &["feature", "routing"],
            &["feature", "routing", "on", "now"],
        ] {
            assert!(parse(refused.iter().copied()).is_err(), "{refused:?}");
        }
        assert!(matches!(
            parse(["--config", "/tmp/s.json", "sign-out"]).unwrap(),
            Command::SignOut {
                config: Some(_),
                demo: false
            }
        ));
        assert!(matches!(
            parse(["--demo", "sign-out"]).unwrap(),
            Command::SignOut { demo: true, .. }
        ));
    }

    #[test]
    fn routing_commands_parse_keys_rules_and_test_sends() {
        use super::IntegrationAction;
        use stardust_core::api::Severity;
        use stardust_core::config::{FieldUpdate, RoutingUpdate};
        let Command::Integration {
            action: IntegrationAction::Key { id, key },
            ..
        } = parse([
            "integration",
            "key",
            "pd",
            "exampleroutingkey000000000000000",
        ])
        .unwrap()
        else {
            panic!("expected key");
        };
        assert_eq!(id, "pd");
        assert_eq!(key, "exampleroutingkey000000000000000");
        let Command::Integration {
            action: IntegrationAction::Route { id, update },
            ..
        } = parse([
            "integration",
            "route",
            "ops",
            "--min-severity",
            "medium",
            "--max-per-hour",
            "12",
            "--quiet-bypass",
            "off",
        ])
        .unwrap()
        else {
            panic!("expected route");
        };
        assert_eq!(id, "ops");
        assert_eq!(
            update,
            RoutingUpdate {
                min_severity: Some(FieldUpdate::Set(Severity::Medium)),
                max_per_hour: Some(FieldUpdate::Set(12)),
                quiet_high_bypass: Some(FieldUpdate::Set(false)),
            }
        );
        let Command::Integration {
            action: IntegrationAction::Route { update, .. },
            ..
        } = parse(["integration", "route", "ops", "--max-per-hour", "default"]).unwrap()
        else {
            panic!("expected route");
        };
        assert_eq!(update.max_per_hour, Some(FieldUpdate::Clear));
        assert_eq!(update.min_severity, None);
        assert!(matches!(
            parse(["integration", "test", "ops"]).unwrap(),
            Command::Integration {
                action: IntegrationAction::Test { .. },
                ..
            }
        ));
        for refused in [
            &["integration", "key", "pd"][..],
            &["integration", "key", "pd", "k", "extra"],
            &["integration", "route", "ops"],
            &["integration", "route", "ops", "--min-severity"],
            &["integration", "route", "ops", "--min-severity", "urgent"],
            &["integration", "route", "ops", "--max-per-hour", "many"],
            &["integration", "route", "ops", "--max-per-hour", "-1"],
            &["integration", "route", "ops", "--quiet-bypass", "maybe"],
            &[
                "integration",
                "route",
                "ops",
                "--min-severity",
                "low",
                "--min-severity",
                "high",
            ],
            &["integration", "route", "ops", "--loud", "on"],
            &["integration", "test"],
            &["integration", "test", "ops", "extra"],
        ] {
            assert!(parse(refused.iter().copied()).is_err(), "{refused:?}");
        }
        let help = usage();
        assert!(help.contains("feature NAME on|off"));
        assert!(help.contains("integration route ID"));
        assert!(help.contains("discord, pagerduty"));
    }
}
