# Security

Stardust is an early preview. Report a vulnerability through GitHub private advisories:

https://github.com/stardusting-labs/stardust/security/advisories/new

Please do not attach access tokens, keychain exports, or config files that contain a cursor you do not want published. A short description and a way to reproduce the issue are enough.

By default the app talks only to `https://helloorbit.app`, or to the server named in config or `STARDUST_SERVER_URL`. The access token stays in the OS keychain for at most 5 minutes and is not written to logs. Links that leave the app must be https on that same host. `--demo` does not open a network connection.
