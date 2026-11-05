# Privacy

This page restates the Privacy section of [README.md](README.md). It adds no new claims.

Stardust does not send telemetry.

Stored on this computer:

- The config file holds the schedule, character position, skin, size, whether the first settings window was closed, integration ids and public hosts, the last checked time, and an optional server address. An older alerts cursor, if one is still in the file, is cleared and is not sent. The file does not hold the access token.
- The OS keychain holds the ORBIT access token and any refresh token, and, only if you turn an integration on, that integration's destination URL, signing secret, or access token.

Network calls, when you are not in `--demo`:

- `POST /api/pqc/v1/account/device/code` and `POST /api/pqc/v1/account/device/token` during sign-in.
- `POST /api/pqc/v1/account/device/token` with the `refresh_token` grant, only when a saved refresh token is used.
- `GET /api/pqc/v1/desktop/status` and `GET /api/pqc/v1/desktop/alerts` on each check, with the keychain token as a bearer header. A 404 with an empty body means those routes are turned off. The alerts response is the current list, with no cursor.
- Optional Slack, Teams, JSON webhook, or MCP calls that you enable.
- Opening your browser to an https page on the configured host when you click a bubble or start sign-in.
