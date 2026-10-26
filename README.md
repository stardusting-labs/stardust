# Stardust

Stardust is an early-preview desktop companion for [ORBIT](https://github.com/stardusting-labs/rust-orbit). It checks ORBIT on a schedule you choose and shows the result in a small speech bubble. A high-severity item also raises an OS notification from that same check.

This is not a chat product. There is no message composer, no conversational assistant, and no language-model call. Choosing a bubble opens the matching ORBIT workspace page in your browser.

The default server is `https://helloorbit.app`. `--demo` uses sample items and does not open a network connection.

## What a check shows

Each check asks ORBIT for desktop status, then alerts, once each. The summary has a count, an optional short title, and the last checked time. The bubble stays up for about six seconds, or until you click it. Clicking opens `https://helloorbit.app` plus the item's workspace path. A path that is not on that https host is not opened. If ORBIT is turned off or cannot be reached, the pet stays calm and the next try waits longer. Stardust does not crash and does not retry in a tight loop.

The character can be shown or hidden. The exact screen position is stored either way. While the character is hidden, only the bubble appears, at that stored position. While the character is shown, the bubble sits just above it. A position that falls outside every monitor is clamped for display and left unchanged in the config file. The bubble window exists only while a result is on screen.

## Pet

The tray pet has two skins. Astronaut is the default. Star is the other. Either skin, and a size of 2x or 3x, can be chosen from the tray menu. The choice is saved in the config file and applies without a restart.

Sprites are nearest-neighbour scaled. At 2x the astronaut is 48px and the star is 64px. At 3x those are 72px and 96px. A status mark sits on the pet's top-right corner. The normal face has no mark.

The face follows the latest check, and whether quiet hours are in effect when the pet is drawn:

- Before the first check, the pet is normal. During quiet hours it sleeps.
- A high-severity item uses the alert face, including during quiet hours.
- A failed check uses the error face.
- A check with nothing new uses the happy face, or sleeps during quiet hours.
- Any other new or changed item uses the alert face, or sleeps during quiet hours.

The tray icon uses the normal, alert, or error picture at 16, 32, or 64 pixels, from the monitor scale. Sleep and happy keep the normal tray picture. Art credits and the full MIT license texts are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Schedule

Timing is yours:

- An interval is elapsed time, from 1 minute through 24 hours (`45m`, `2h`, `90`). The tray menu offers 1, 30, and 60 minutes as shortcuts.
- Clock times are civil times in the schedule timezone, such as `09:00`, `13:00`, and `18:00`.
- Weekdays limit both kinds of slot. The default is every day.
- Quiet hours hide ordinary results. A high-severity item from a scheduled check is still shown. Quiet hours do not add or remove checks.

The next check is the earlier of the next interval step and the next clock time. The tray menu shows that time. A clock time does not move the interval anchor. A missed slot is not replayed. If ORBIT is off or cannot be reached, the next try waits at least 30 seconds, then doubles up to 32 minutes, and never comes sooner than the next slot.

An interval counts absolute elapsed time, including across a daylight-saving change. A clock time that does not exist on a spring-forward day is skipped. A clock time that occurs twice on a fall-back day is used once, at the earlier offset.

The first check waits until the next slot. Check now, in the tray menu, runs one extra check and does not move the interval anchor.

## Install

Release archives are unsigned. They are built by the `release-build` GitHub Actions workflow, which runs only when someone starts it by hand. The workflow uploads the archives and a `SHA256SUMS` file as workflow artifacts. It does not create a GitHub release.

- Linux: `stardust-linux-x86_64.tar.gz`. Unpack it and run `stardust`. The machine needs the usual Wayland or X11 libraries (`libxkbcommon`, `libwayland-client`, `libx11`).
- macOS: `stardust-macos-universal.zip`, an unsigned universal `.app`. The first open can be blocked. Use the Finder shortcut to open it, or allow it under Privacy & Security.
- Windows: `stardust-windows-x86_64.zip`. Unpack it and run `stardust.exe`. SmartScreen can warn because the file is unsigned.

Compare the archive checksum with `SHA256SUMS` before you run it.

## Sign-in

```text
stardust sign-in
stardust sign-out
```

`sign-in` asks ORBIT for a device code, prints a short user code, and opens the ORBIT account page when that page is https on the configured host. Approve the code in the browser. Stardust polls until the code is approved or it expires, then stores the access token in the OS keychain. The token is not printed, and it is not written into the config file. `sign-out` deletes that keychain item.

The server is `https://helloorbit.app` unless `server_base_url` is set in the config file or `STARDUST_SERVER_URL` is set in the environment. The environment value wins. A 401 clears the saved token and asks you to sign in again. A 404, or a server that cannot be reached, shows a calm note and retries with backoff.

`--demo` skips sign-in and shows sample items.

## Run

```text
stardust
stardust --demo
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
  "pet_skin": "astronaut",
  "pet_scale": 2,
  "schedule": {
    "interval_minutes": 45,
    "clock_times": ["09:00", "13:00", "18:00"],
    "weekdays": ["mon", "tue", "wed", "thu", "fri"],
    "quiet_hours": "22:00-07:00",
    "timezone": "America/New_York"
  },
  "integrations": []
}
```

An ORBIT access token is kept in the system keychain until the server expiry, and never longer than 5 minutes. Stardust will not store one that is already expired. The token is not written into the config file. Sign in again after it expires. There is no refresh token.

## Integrations

Every integration is off until you enable it. Slack, Teams, a signed JSON webhook, an ORBIT MCP client, and Slack MCP are the first catalog entries. Later entries can use the same on/off switch and keychain slots.

Slack, Teams, and JSON posts go out from the check that fills the speech bubble. A high-severity item is posted from that check, including during quiet hours. Other results are posted on a scheduled check outside quiet hours. There is no second poll. A schedule that has clock times and no interval reports 1440 minutes in the webhook field, because that field requires a minute count.

The JSON body matches [ORBIT's outbound webhook event draft](https://github.com/stardusting-labs/rust-orbit/blob/main/docs/integrations/mcp-and-webhooks.md). That draft does not include an MCP server. The ORBIT MCP entry stays off until you save an https endpoint and a keychain bearer. `integration connect` then sends `initialize` and `tools/list`. Stardust does not invent tools and does not call one on a timer.

Slack MCP is optional. `integration oauth` signs in through Slack's hosted MCP at `https://mcp.slack.com/mcp` and stores the token in the keychain. Stardust does not call a send tool on its own. The Slack app client id is not bundled.

Destination URLs, signing secrets, and access tokens are keychain items. The config file keeps the integration id, kind, on/off switch, a host name, a public MCP endpoint, and a public client id. Preview tenant and workspace ids are fixed until sign-in exists.

```text
stardust integration list
stardust integration add json ops
stardust integration url ops https://helloorbit.app/hooks
stardust integration enable ops
stardust integration add slack-mcp slack
stardust integration oauth slack --client-id CLIENT_ID
stardust integration connect slack
```

## Privacy

Stardust does not send telemetry.

Stored on this computer:

- The config file holds the schedule, character position, skin, size, integration ids and public hosts, the last checked time, and an optional server address. An older alerts cursor, if one is still in the file, is cleared and is not sent. The file does not hold the access token.
- The OS keychain holds the ORBIT access token, and, only if you turn an integration on, that integration's destination URL, signing secret, or access token.

Network calls, when you are not in `--demo`:

- `POST /api/pqc/v1/account/device/code` and `POST /api/pqc/v1/account/device/token` during sign-in.
- `GET /api/pqc/v1/desktop/status` and `GET /api/pqc/v1/desktop/alerts` on each check, with the keychain token as a bearer header. A 404 with an empty body means those routes are turned off. The alerts response is the current list, with no cursor.
- Optional Slack, Teams, JSON webhook, or MCP calls that you enable.
- Opening your browser to an https page on the configured host when you click a bubble or start sign-in.

## Dependencies

No font file is bundled. The on-screen letters are an original bitmap, so no OFL font is included. No government dataset is bundled. Check results still come from ORBIT. Pet and tray pictures are bundled. Credits and the MIT license texts are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Direct crates:

| Crate | Version | License |
| --- | --- | --- |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| uuid | 1.28.0 | Apache-2.0 OR MIT |
| ureq | 2.12.1 | MIT OR Apache-2.0 |
| png | 0.18.1 | MIT OR Apache-2.0 |

`uuid` was already in the tree. `sha2` 0.11.1 was already in the tree; 0.10.9 is the version this crate calls. `png` 0.18.1 was already in the tree through the tray icon. This crate calls it, with default features off, to decode the pet sprites. No new transitive crates were added. No new crate was added for the ORBIT client. It uses `ureq` 2.12.1, which was already the HTTPS client. The HTTPS client adds these crates:

| Crate | Version | License |
| --- | --- | --- |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 |
| generic-array | 0.14.7 | MIT |
| getrandom | 0.2.17 | MIT OR Apache-2.0 |
| icu_collections | 2.4.0 | Unicode-3.0 |
| icu_locale_core | 2.4.0 | Unicode-3.0 |
| icu_normalizer | 2.4.0 | Unicode-3.0 |
| icu_normalizer_data | 2.4.0 | Unicode-3.0 |
| icu_properties | 2.4.0 | Unicode-3.0 |
| icu_properties_data | 2.4.0 | Unicode-3.0 |
| icu_provider | 2.4.0 | Unicode-3.0 |
| idna | 1.1.0 | MIT OR Apache-2.0 |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT |
| litemap | 0.8.3 | Unicode-3.0 |
| potential_utf | 0.1.6 | Unicode-3.0 |
| ring | 0.17.14 | Apache-2.0 AND ISC |
| rustls | 0.23.46 | Apache-2.0 OR ISC OR MIT |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 |
| rustls-webpki | 0.103.16 | ISC |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 |
| subtle | 2.6.1 | BSD-3-Clause |
| synstructure | 0.14.0 | MIT |
| tinystr | 0.8.5 | Unicode-3.0 |
| untrusted | 0.9.0 | ISC |
| url | 2.5.8 | MIT OR Apache-2.0 |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT |
| wasi | 0.11.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| webpki-roots | 0.26.11 and 1.0.10 | CDLA-Permissive-2.0 |
| writeable | 0.6.4 | Unicode-3.0 |
| yoke | 0.8.4 | Unicode-3.0 |
| yoke-derive | 0.8.4 | Unicode-3.0 |
| zerofrom | 0.1.8 | Unicode-3.0 |
| zerofrom-derive | 0.1.8 | Unicode-3.0 |
| zerotrie | 0.2.5 | Unicode-3.0 |
| zerovec | 0.11.9 | Unicode-3.0 |
| zerovec-derive | 0.11.7 | Unicode-3.0 |

## Idle memory

Measured on Linux x86_64 with the release binary, the character hidden, a 24 hour interval, no speech bubble, every integration off, and the tray icon installed. Five `VmRSS` samples were read from `/proc/<pid>/status` after a short warmup. The median was 9.1 MiB (9276 KiB). `scripts/measure-idle.sh` repeats that sample.

## License

Apache-2.0. See [LICENSE](LICENSE), [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), and [SECURITY.md](SECURITY.md).
