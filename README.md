# Stardust

Stardust is an early-preview desktop companion for [ORBIT](https://github.com/stardusting-labs/rust-orbit). It checks ORBIT on a schedule you choose and shows the result in a small speech bubble. A high-severity item also raises an OS notification from that same check.

This is not a chat product. There is no message composer, no conversational assistant, and no language-model call. Choosing a bubble opens the ORBIT workspace page in your browser.

The desktop API used here is proposed and is not served yet. The app talks to a built-in mock and does not open a network connection. Placeholder links use `https://helloorbit.app`.

## What a check shows

Each check asks the alerts endpoint once. The summary has a count, an optional short title, and the last checked time. The bubble stays up for about six seconds, or until you click it. Clicking opens the first high-severity http(s) link, otherwise the first http(s) link, otherwise `https://helloorbit.app`.

The character can be shown or hidden. The exact screen position is stored either way. While the character is hidden, only the bubble appears, at that stored position. While the character is shown, the bubble sits just above it. A position that falls outside every monitor is clamped for display and left unchanged in the config file. The bubble window exists only while a result is on screen.

## Schedule

Timing is yours:

- An interval is elapsed time, from 1 minute through 24 hours (`45m`, `2h`, `90`). The tray menu offers 1, 30, and 60 minutes as shortcuts.
- Clock times are civil times in the schedule timezone, such as `09:00`, `13:00`, and `18:00`.
- Weekdays limit both kinds of slot. The default is every day.
- Quiet hours hide ordinary results. A high-severity item from a scheduled check is still shown. Quiet hours do not add or remove checks.

The next check is the earlier of the next interval step and the next clock time. The tray menu shows that time. A clock time does not move the interval anchor. A missed slot is not replayed. If a check fails, the following poll is still the next slot.

An interval counts absolute elapsed time, including across a daylight-saving change. A clock time that does not exist on a spring-forward day is skipped. A clock time that occurs twice on a fall-back day is used once, at the earlier offset.

The first check waits until the next slot. Check now, in the tray menu, runs one extra check and does not move the interval anchor.

## Run

```text
stardust
stardust --check-once
stardust schedule show
stardust schedule set --interval 45m --time 09:00 --time 13:00 --time 18:00 --weekdays mon-fri --quiet 22:00-07:00
stardust character hide
stardust character position 64 64
```

Config is JSON. On Linux it is `$XDG_CONFIG_HOME/stardust/config.json` or `~/.config/stardust/config.json`.

```json
{
  "character_visible": false,
  "character_position": { "x": 64, "y": 64 },
  "schedule": {
    "interval_minutes": 45,
    "clock_times": ["09:00", "13:00", "18:00"],
    "weekdays": ["mon", "tue", "wed", "thu", "fri"],
    "quiet_hours": "22:00-07:00",
    "timezone": "America/New_York"
  }
}
```

A short-lived ORBIT access token can be kept in the system keychain. This preview does not sign in, and the token lifetime is capped at 15 minutes. The token is not written into the config file.

## Idle memory

Measured on Linux x86_64 with the release binary, the character hidden, a 24 hour interval, no speech bubble, and the tray icon installed. Five `VmRSS` samples were read from `/proc/<pid>/status` after a short warmup. The median was 8.6 MiB (8852 KiB). `scripts/measure-idle.sh` repeats that sample.

## License

Apache-2.0. See [LICENSE](LICENSE) and [SECURITY.md](SECURITY.md).
