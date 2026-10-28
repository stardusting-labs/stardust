//! User-defined check schedule.
//!
//! An interval is elapsed time. A clock time is the civil time in the
//! schedule's timezone. When that civil time is missing, that day is skipped.
//! When it happens twice, only the earlier instant is used. Quiet hours do
//! not add or remove polls.

use chrono::{
    DateTime, Datelike, Days, LocalResult, NaiveDate, NaiveTime, TimeDelta, Timelike, Weekday,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Tray suggestions. A custom interval may be any other whole minute in range.
pub const QUICK_PICK_MINUTES: [u32; 3] = [1, 30, 60];

const MIN_MINUTES: u32 = 1;
const MAX_MINUTES: u32 = 24 * 60;
const MAX_CLOCK_TIMES: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("could not read the interval")]
    IntervalFormat,
    #[error("interval must be from 1 minute through 24 hours")]
    IntervalRange,
    #[error("could not read the clock time {0}")]
    Clock(String),
    #[error("too many clock times")]
    TooManyTimes,
    #[error("could not read the weekdays")]
    Weekdays,
    #[error("unknown weekday {0}")]
    Weekday(String),
    #[error("could not read the quiet hours")]
    QuietFormat,
    #[error("quiet hours need a start and end that differ")]
    QuietRange,
    #[error("unknown timezone {0}")]
    Timezone(String),
    #[error("set an interval or at least one clock time")]
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weekdays(u8);

impl Weekdays {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0b0111_1111);

    pub fn with(self, day: Weekday) -> Self {
        Self(self.0 | day_bit(day))
    }

    pub fn contains(self, day: Weekday) -> bool {
        self.0 & day_bit(day) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn days(self) -> impl Iterator<Item = Weekday> {
        [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ]
        .into_iter()
        .filter(move |day| self.contains(*day))
    }

    pub fn label(self) -> String {
        if self == Self::ALL {
            return "all days".to_string();
        }
        let days: Vec<Weekday> = self.days().collect();
        if let Some(label) = contiguous_label(&days) {
            return label;
        }
        if days.is_empty() {
            return "no days".to_string();
        }
        days.iter()
            .map(|day| short_day(*day))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn contiguous_label(days: &[Weekday]) -> Option<String> {
    if days.len() < 2 {
        return None;
    }
    let nums: Vec<u32> = days.iter().map(Weekday::num_days_from_monday).collect();
    let contiguous = nums.windows(2).all(|pair| pair[1] == pair[0] + 1);
    let last = *days.last()?;
    contiguous.then(|| format!("{}-{}", short_day(days[0]), short_day(last)))
}

fn day_bit(day: Weekday) -> u8 {
    1u8.checked_shl(day.num_days_from_monday()).unwrap_or(0)
}

fn short_day(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

pub fn day_token(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

pub fn parse_weekdays(input: &str) -> Result<Weekdays, ScheduleError> {
    let text = input.trim().to_ascii_lowercase();
    if text.is_empty() || text.len() > 128 {
        return Err(ScheduleError::Weekdays);
    }
    if text == "all" || text == "everyday" || text == "every day" {
        return Ok(Weekdays::ALL);
    }
    let mut set = Weekdays::NONE;
    for part in text.split(|c: char| c == ',' || c.is_whitespace()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            set = set.union(weekday_range(parse_day(start)?, parse_day(end)?));
        } else {
            set = set.with(parse_day(part)?);
        }
    }
    if set.is_empty() {
        return Err(ScheduleError::Weekdays);
    }
    Ok(set)
}

fn parse_day(value: &str) -> Result<Weekday, ScheduleError> {
    match value.trim() {
        "mon" | "monday" => Ok(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Ok(Weekday::Tue),
        "wed" | "weds" | "wednesday" => Ok(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Ok(Weekday::Thu),
        "fri" | "friday" => Ok(Weekday::Fri),
        "sat" | "saturday" => Ok(Weekday::Sat),
        "sun" | "sunday" => Ok(Weekday::Sun),
        other => Err(ScheduleError::Weekday(other.to_string())),
    }
}

fn weekday_range(start: Weekday, end: Weekday) -> Weekdays {
    let mut set = Weekdays::NONE;
    let mut day = start;
    for _ in 0..7 {
        set = set.with(day);
        if day == end {
            break;
        }
        day = day.succ();
    }
    set
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    start: NaiveTime,
    end: NaiveTime,
}

impl QuietHours {
    pub fn try_new(start: NaiveTime, end: NaiveTime) -> Result<Self, ScheduleError> {
        if start == end {
            return Err(ScheduleError::QuietRange);
        }
        Ok(Self { start, end })
    }

    pub fn contains(self, time: NaiveTime) -> bool {
        if self.start < self.end {
            time >= self.start && time < self.end
        } else {
            time >= self.start || time < self.end
        }
    }

    pub fn label(self) -> String {
        format!("{}-{}", format_time(self.start), format_time(self.end))
    }
}

pub fn parse_quiet(input: &str) -> Result<QuietHours, ScheduleError> {
    let text = input.trim().replace(['–', '—'], "-");
    if text.len() > 32 {
        return Err(ScheduleError::QuietFormat);
    }
    let (start, end) = text.split_once('-').ok_or(ScheduleError::QuietFormat)?;
    QuietHours::try_new(parse_clock(start)?, parse_clock(end)?)
}

pub fn parse_clock(input: &str) -> Result<NaiveTime, ScheduleError> {
    let text = input.trim();
    if text.is_empty() || text.len() > 8 {
        return Err(ScheduleError::Clock(text.to_string()));
    }
    let Some((hour, minute)) = text.split_once(':') else {
        return Err(ScheduleError::Clock(text.to_string()));
    };
    if hour.is_empty()
        || hour.len() > 2
        || minute.len() != 2
        || !hour.chars().all(|c| c.is_ascii_digit())
        || !minute.chars().all(|c| c.is_ascii_digit())
    {
        return Err(ScheduleError::Clock(text.to_string()));
    }
    let hour: u32 = hour
        .parse()
        .map_err(|_| ScheduleError::Clock(text.to_string()))?;
    let minute: u32 = minute
        .parse()
        .map_err(|_| ScheduleError::Clock(text.to_string()))?;
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(|| ScheduleError::Clock(text.to_string()))
}

pub fn parse_interval(input: &str) -> Result<u32, ScheduleError> {
    let text = input.trim().to_ascii_lowercase();
    if text.is_empty() || text.len() > 32 {
        return Err(ScheduleError::IntervalFormat);
    }
    let (number, unit) = split_number(&text).ok_or(ScheduleError::IntervalFormat)?;
    let unit = unit.trim();
    let minutes = if unit.is_empty() || matches!(unit, "m" | "min" | "mins" | "minute" | "minutes")
    {
        parse_whole_minutes(number)?
    } else if matches!(unit, "h" | "hr" | "hrs" | "hour" | "hours") {
        parse_hours(number)?
    } else {
        return Err(ScheduleError::IntervalFormat);
    };
    if (MIN_MINUTES..=MAX_MINUTES).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(ScheduleError::IntervalRange)
    }
}

fn split_number(text: &str) -> Option<(&str, &str)> {
    let mut dots = 0;
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            end = index + 1;
            continue;
        }
        if ch == '.' && dots == 0 {
            dots += 1;
            end = index + 1;
            continue;
        }
        break;
    }
    if end == 0 {
        return None;
    }
    Some((&text[..end], &text[end..]))
}

fn parse_whole_minutes(number: &str) -> Result<u32, ScheduleError> {
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return Err(ScheduleError::IntervalFormat);
    }
    number.parse().map_err(|_| ScheduleError::IntervalFormat)
}

fn parse_hours(number: &str) -> Result<u32, ScheduleError> {
    let (whole, frac) = match number.split_once('.') {
        Some((whole, frac)) => (whole, frac),
        None => (number, ""),
    };
    if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit()) {
        return Err(ScheduleError::IntervalFormat);
    }
    if frac.len() > 4 || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(ScheduleError::IntervalFormat);
    }
    let hours: u32 = whole.parse().map_err(|_| ScheduleError::IntervalFormat)?;
    let base = hours.checked_mul(60).ok_or(ScheduleError::IntervalRange)?;
    if frac.is_empty() {
        return Ok(base);
    }
    let numer: u32 = frac.parse().map_err(|_| ScheduleError::IntervalFormat)?;
    let denom = 10u32.pow(u32::try_from(frac.len()).unwrap_or(u32::MAX));
    let scaled = numer.checked_mul(60).ok_or(ScheduleError::IntervalRange)?;
    if scaled % denom != 0 {
        return Err(ScheduleError::IntervalFormat);
    }
    base.checked_add(scaled / denom)
        .ok_or(ScheduleError::IntervalRange)
}

pub fn format_interval(minutes: u32) -> String {
    if minutes == 1 {
        "1 minute".to_string()
    } else {
        format!("{minutes} minutes")
    }
}

pub fn format_time(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

pub fn parse_timezone(name: &str) -> Result<Tz, ScheduleError> {
    Tz::from_str(name).map_err(|_| ScheduleError::Timezone(name.to_string()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireSource {
    Interval,
    Clock,
    Both,
}

impl FireSource {
    pub fn updates_interval_anchor(self) -> bool {
        matches!(self, Self::Interval | Self::Both)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fire {
    pub at: DateTime<Tz>,
    pub source: FireSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    interval_minutes: Option<u32>,
    clock_times: Vec<NaiveTime>,
    weekdays: Weekdays,
    quiet: Option<QuietHours>,
    timezone: Tz,
}

impl Schedule {
    pub fn try_new(
        interval_minutes: Option<u32>,
        mut clock_times: Vec<NaiveTime>,
        weekdays: Weekdays,
        quiet: Option<QuietHours>,
        timezone: Tz,
    ) -> Result<Self, ScheduleError> {
        if let Some(minutes) = interval_minutes {
            if !(MIN_MINUTES..=MAX_MINUTES).contains(&minutes) {
                return Err(ScheduleError::IntervalRange);
            }
        }
        clock_times.sort();
        clock_times.dedup();
        if clock_times.len() > MAX_CLOCK_TIMES {
            return Err(ScheduleError::TooManyTimes);
        }
        if interval_minutes.is_none() && clock_times.is_empty() {
            return Err(ScheduleError::Empty);
        }
        if weekdays.is_empty() {
            return Err(ScheduleError::Weekdays);
        }
        Ok(Self {
            interval_minutes,
            clock_times,
            weekdays,
            quiet,
            timezone,
        })
    }

    pub fn interval_minutes(&self) -> Option<u32> {
        self.interval_minutes
    }

    pub fn clock_times(&self) -> &[NaiveTime] {
        &self.clock_times
    }

    pub fn weekdays(&self) -> Weekdays {
        self.weekdays
    }

    pub fn quiet_hours(&self) -> Option<QuietHours> {
        self.quiet
    }

    pub fn timezone(&self) -> Tz {
        self.timezone
    }

    pub fn is_quiet_at(&self, local: DateTime<Tz>) -> bool {
        let local = local.with_timezone(&self.timezone);
        self.quiet.is_some_and(|quiet| quiet.contains(local.time()))
    }

    pub fn describe(&self) -> String {
        let when = match (self.interval_minutes, self.clock_times.is_empty()) {
            (Some(minutes), true) => format!("every {}", format_interval(minutes)),
            (Some(minutes), false) => {
                format!(
                    "every {} and at {}",
                    format_interval(minutes),
                    self.clock_label()
                )
            }
            (None, false) => format!("at {}", self.clock_label()),
            (None, true) => "unscheduled".to_string(),
        };
        let mut text = format!("{when} on {}", self.weekdays.label());
        if let Some(quiet) = self.quiet {
            text.push_str(", quiet hours ");
            text.push_str(&quiet.label());
        }
        text
    }

    fn clock_label(&self) -> String {
        self.clock_times
            .iter()
            .map(|time| format_time(*time))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Next poll strictly after `after`. Missed slots are not returned.
    pub fn next_fire(
        &self,
        after: DateTime<Tz>,
        interval_anchor: Option<DateTime<Tz>>,
    ) -> Option<Fire> {
        let after = after.with_timezone(&self.timezone);
        let anchor = interval_anchor.map(|instant| instant.with_timezone(&self.timezone));
        let interval_at = self.next_interval(after, anchor);
        let clock_at = next_clock(after, &self.clock_times, self.weekdays, self.timezone);
        match (interval_at, clock_at) {
            (Some(interval), Some(clock)) if interval == clock => Some(Fire {
                at: interval,
                source: FireSource::Both,
            }),
            (Some(interval), Some(clock)) if interval < clock => Some(Fire {
                at: interval,
                source: FireSource::Interval,
            }),
            (Some(_interval), Some(clock)) => Some(Fire {
                at: clock,
                source: FireSource::Clock,
            }),
            (Some(interval), None) => Some(Fire {
                at: interval,
                source: FireSource::Interval,
            }),
            (None, Some(clock)) => Some(Fire {
                at: clock,
                source: FireSource::Clock,
            }),
            (None, None) => None,
        }
    }

    pub fn upcoming_fires(
        &self,
        after: DateTime<Tz>,
        mut anchor: Option<DateTime<Tz>>,
        limit: usize,
    ) -> Vec<Fire> {
        let mut fires = Vec::new();
        let mut cursor = after.with_timezone(&self.timezone);
        for _ in 0..limit {
            let Some(fire) = self.next_fire(cursor, anchor) else {
                break;
            };
            if fire.at <= cursor {
                break;
            }
            if fire.source.updates_interval_anchor() {
                anchor = Some(fire.at);
            }
            cursor = fire.at;
            fires.push(fire);
        }
        fires
    }

    fn next_interval(
        &self,
        after: DateTime<Tz>,
        anchor: Option<DateTime<Tz>>,
    ) -> Option<DateTime<Tz>> {
        let minutes = self.interval_minutes?;
        let step = TimeDelta::minutes(i64::from(minutes));
        let anchor = anchor.unwrap_or(after);
        let mut candidate = first_after(anchor, after, step)?;
        for _ in 0..20_000 {
            if candidate > after && self.weekdays.contains(candidate.weekday()) {
                return Some(candidate);
            }
            candidate = candidate.checked_add_signed(step)?;
        }
        None
    }
}

fn first_after(anchor: DateTime<Tz>, after: DateTime<Tz>, step: TimeDelta) -> Option<DateTime<Tz>> {
    if anchor > after {
        return Some(anchor);
    }
    let step_ms = step.num_milliseconds();
    if step_ms <= 0 {
        return None;
    }
    let elapsed_ms = after.signed_duration_since(anchor).num_milliseconds();
    let steps = elapsed_ms.div_euclid(step_ms) + 1;
    add_steps(anchor, step, steps)
}

fn add_steps(anchor: DateTime<Tz>, step: TimeDelta, steps: i64) -> Option<DateTime<Tz>> {
    let steps = i32::try_from(steps).ok()?;
    let delta = step.checked_mul(steps)?;
    anchor.checked_add_signed(delta)
}

fn next_clock(
    after: DateTime<Tz>,
    times: &[NaiveTime],
    days: Weekdays,
    tz: Tz,
) -> Option<DateTime<Tz>> {
    if times.is_empty() {
        return None;
    }
    let start = after.date_naive();
    for offset in 0..14 {
        let date = start.checked_add_days(Days::new(offset))?;
        if !days.contains(date.weekday()) {
            continue;
        }
        for time in times {
            if let Some(instant) = resolve_local(date, *time, tz) {
                if instant > after {
                    return Some(instant);
                }
            }
        }
    }
    None
}

fn resolve_local(date: NaiveDate, time: NaiveTime, tz: Tz) -> Option<DateTime<Tz>> {
    match date.and_time(time).and_local_timezone(tz) {
        LocalResult::Single(instant) => Some(instant),
        LocalResult::Ambiguous(early, _late) => Some(early),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_interval, parse_clock, parse_interval, parse_quiet, parse_weekdays, FireSource,
        QuietHours, Schedule, Weekdays, QUICK_PICK_MINUTES,
    };
    use chrono::{Datelike, LocalResult, TimeZone, Weekday};
    use chrono_tz::{America::New_York, Tz, UTC};

    fn at(tz: Tz, year: i32, month: u32, day: u32, hour: u32, minute: u32) -> chrono::DateTime<Tz> {
        match tz.with_ymd_and_hms(year, month, day, hour, minute, 0) {
            LocalResult::Single(instant) => instant,
            other => panic!("expected one instant, got {other:?}"),
        }
    }

    fn sched(
        interval: Option<u32>,
        times: &[&str],
        days: &str,
        quiet: Option<&str>,
        tz: Tz,
    ) -> Schedule {
        let times = times
            .iter()
            .map(|time| parse_clock(time).unwrap())
            .collect();
        let quiet = quiet.map(|text| parse_quiet(text).unwrap());
        Schedule::try_new(interval, times, parse_weekdays(days).unwrap(), quiet, tz).unwrap()
    }

    #[test]
    fn parses_intervals_clock_times_weekdays_and_quiet_hours() {
        for (input, minutes) in [
            ("1", 1),
            ("1m", 1),
            ("30 min", 30),
            ("60 minutes", 60),
            ("90m", 90),
            ("2h", 120),
            ("1.5h", 90),
            ("0.5 hours", 30),
            ("24h", 1440),
            ("1440", 1440),
        ] {
            assert_eq!(parse_interval(input).unwrap(), minutes, "{input}");
        }
        for input in ["0", "0m", "25h", "1441", "30s", "", "1.5", "abc", "-5"] {
            assert!(parse_interval(input).is_err(), "{input}");
        }
        assert_eq!(parse_clock("09:00").unwrap(), parse_clock("9:00").unwrap());
        assert!(parse_clock("24:00").is_err());
        assert!(parse_clock("12:60").is_err());
        assert_eq!(parse_weekdays("mon-fri").unwrap().label(), "Mon-Fri");
        assert_eq!(parse_weekdays("all").unwrap(), Weekdays::ALL);
        assert!(parse_weekdays("").is_err());
        assert!(parse_weekdays("fundays").is_err());
        let quiet = parse_quiet("22:00-07:00").unwrap();
        assert!(quiet.contains(parse_clock("23:00").unwrap()));
        assert!(quiet.contains(parse_clock("06:59").unwrap()));
        assert!(quiet.contains(parse_clock("22:00").unwrap()));
        assert!(!quiet.contains(parse_clock("07:00").unwrap()));
        assert!(!quiet.contains(parse_clock("12:00").unwrap()));
        let daytime = parse_quiet("09:00–17:00").unwrap();
        assert!(daytime.contains(parse_clock("09:00").unwrap()));
        assert!(daytime.contains(parse_clock("16:59").unwrap()));
        assert!(!daytime.contains(parse_clock("17:00").unwrap()));
        assert!(
            QuietHours::try_new(parse_clock("10:00").unwrap(), parse_clock("10:00").unwrap())
                .is_err()
        );
        assert_eq!(QUICK_PICK_MINUTES, [1, 30, 60]);
        assert_eq!(format_interval(1), "1 minute");
        assert_eq!(format_interval(60), "60 minutes");
    }

    #[test]
    fn wrapped_weekday_range_includes_the_weekend() {
        let days = parse_weekdays("fri-mon").unwrap();
        assert!(days.contains(Weekday::Fri));
        assert!(days.contains(Weekday::Sat));
        assert!(days.contains(Weekday::Sun));
        assert!(days.contains(Weekday::Mon));
        assert!(!days.contains(Weekday::Tue));
    }

    #[test]
    fn missed_interval_slots_are_not_replayed() {
        let anchor = at(UTC, 2026, 10, 21, 0, 0);
        let schedule = sched(Some(60), &[], "all", None, UTC);
        let after = at(UTC, 2026, 10, 21, 5, 30);
        let fire = schedule.next_fire(after, Some(anchor)).unwrap();
        assert_eq!(fire.at, at(UTC, 2026, 10, 21, 6, 0));
        let fires = schedule.upcoming_fires(after, Some(anchor), 3);
        assert_eq!(
            fires.iter().map(|fire| fire.at).collect::<Vec<_>>(),
            vec![
                at(UTC, 2026, 10, 21, 6, 0),
                at(UTC, 2026, 10, 21, 7, 0),
                at(UTC, 2026, 10, 21, 8, 0),
            ]
        );
    }

    #[test]
    fn interval_on_weekdays_uses_the_next_allowed_elapsed_step() {
        let anchor = at(UTC, 2026, 10, 23, 20, 0);
        assert_eq!(anchor.weekday(), Weekday::Fri);
        let schedule = sched(Some(600), &[], "mon-fri", None, UTC);
        let fire = schedule.next_fire(anchor, Some(anchor)).unwrap();
        assert_eq!(fire.at, at(UTC, 2026, 10, 26, 8, 0));
        assert_eq!(fire.at.weekday(), Weekday::Mon);
        assert_eq!(fire.source, FireSource::Interval);
    }

    #[test]
    fn clock_time_skips_to_the_next_allowed_weekday() {
        let schedule = sched(None, &["09:00"], "mon-fri", None, UTC);
        let friday = at(UTC, 2026, 10, 23, 10, 0);
        assert_eq!(friday.weekday(), Weekday::Fri);
        let fire = schedule.next_fire(friday, None).unwrap();
        assert_eq!(fire.at, at(UTC, 2026, 10, 26, 9, 0));
        assert_eq!(fire.source, FireSource::Clock);
    }

    #[test]
    fn earlier_of_interval_and_clock_wins_without_moving_the_anchor_early() {
        let anchor = at(UTC, 2026, 10, 23, 15, 0);
        let schedule = sched(Some(24 * 60), &["09:00"], "mon-fri", None, UTC);
        let fires = schedule.upcoming_fires(anchor, Some(anchor), 2);
        assert_eq!(fires[0].at, at(UTC, 2026, 10, 26, 9, 0));
        assert_eq!(fires[0].source, FireSource::Clock);
        assert_eq!(fires[1].at, at(UTC, 2026, 10, 26, 15, 0));
        assert_eq!(fires[1].source, FireSource::Interval);
    }

    #[test]
    fn matching_interval_and_clock_are_one_poll() {
        let anchor = at(UTC, 2026, 10, 21, 8, 0);
        let schedule = sched(Some(60), &["09:00"], "all", None, UTC);
        let fire = schedule.next_fire(anchor, Some(anchor)).unwrap();
        assert_eq!(fire.at, at(UTC, 2026, 10, 21, 9, 0));
        assert_eq!(fire.source, FireSource::Both);
    }

    #[test]
    fn quiet_hours_do_not_add_or_remove_polls() {
        let start = at(UTC, 2026, 10, 21, 0, 0);
        let plain = sched(Some(60), &["09:00"], "all", None, UTC);
        let quiet = sched(Some(60), &["09:00"], "all", Some("22:00-07:00"), UTC);
        assert_eq!(
            plain.upcoming_fires(start, Some(start), 30),
            quiet.upcoming_fires(start, Some(start), 30)
        );
    }

    #[test]
    fn interval_across_spring_forward_uses_elapsed_time() {
        let anchor = at(New_York, 2026, 3, 8, 1, 30);
        let schedule = sched(Some(60), &[], "all", None, New_York);
        let fire = schedule.next_fire(anchor, Some(anchor)).unwrap();
        assert_eq!(fire.at.timestamp(), anchor.timestamp() + 3600);
        assert_eq!(fire.at, at(New_York, 2026, 3, 8, 3, 30));
    }

    #[test]
    fn missing_local_time_is_skipped_to_the_next_day() {
        assert!(matches!(
            New_York.with_ymd_and_hms(2026, 3, 8, 2, 30, 0),
            LocalResult::None
        ));
        let schedule = sched(None, &["02:30"], "all", None, New_York);
        let after = at(New_York, 2026, 3, 8, 1, 0);
        let fire = schedule.next_fire(after, None).unwrap();
        assert_eq!(fire.at, at(New_York, 2026, 3, 9, 2, 30));
    }

    #[test]
    fn repeated_local_time_fires_once_at_the_earlier_offset() {
        let LocalResult::Ambiguous(early, late) = New_York.with_ymd_and_hms(2026, 11, 1, 1, 30, 0)
        else {
            panic!("expected the fold");
        };
        assert!(early < late);
        let schedule = sched(None, &["01:30"], "all", None, New_York);
        let first = schedule
            .next_fire(at(New_York, 2026, 11, 1, 0, 30), None)
            .unwrap();
        assert_eq!(first.at, early);
        let second = schedule.next_fire(early, None).unwrap();
        assert_eq!(second.at, at(New_York, 2026, 11, 2, 1, 30));
        assert_ne!(second.at, late);
    }

    #[test]
    fn hourly_interval_crosses_both_instants_of_a_repeated_hour() {
        let anchor = at(New_York, 2026, 11, 1, 0, 30);
        let schedule = sched(Some(60), &[], "all", None, New_York);
        let fires = schedule.upcoming_fires(anchor, Some(anchor), 2);
        let LocalResult::Ambiguous(early, late) = New_York.with_ymd_and_hms(2026, 11, 1, 1, 30, 0)
        else {
            panic!("expected the fold");
        };
        assert_eq!(fires[0].at, early);
        assert_eq!(fires[1].at, late);
        assert_eq!(fires[0].at.timestamp() + 3600, fires[1].at.timestamp());
    }

    #[test]
    fn quiet_hours_follow_the_local_clock_during_a_fold() {
        let schedule = sched(Some(60), &[], "all", Some("01:00-02:00"), New_York);
        let LocalResult::Ambiguous(early, late) = New_York.with_ymd_and_hms(2026, 11, 1, 1, 30, 0)
        else {
            panic!("expected the fold");
        };
        assert!(schedule.is_quiet_at(early));
        assert!(schedule.is_quiet_at(late));
        assert!(!schedule.is_quiet_at(at(New_York, 2026, 11, 1, 3, 0)));
    }

    #[test]
    fn empty_schedule_is_rejected() {
        let error = Schedule::try_new(None, Vec::new(), Weekdays::ALL, None, UTC);
        assert!(error.is_err());
    }
}
