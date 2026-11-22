# Launch sign-in window: design

Date: 2026-11-05. Status: built behind the default-off `launch-sign-in-window` flag on branch `feat/launch-signin-window-1105`, not merged and not turned on for anyone. Sections 1 to 8 are the design as written. Section 9 lists where the build differs from it.

Goal, from the owner: starting Stardust without a session shows a sign-in window. Everything sits behind a new feature flag that is off by default.

Paths are relative to the repository root. Line numbers refer to commit `6bcf9a7`.

## 1. Current flow

### What happens at launch

1. `run_loop` loads the config and starts the check-timer thread (`crates/stardust/src/app.rs:215`).
2. The timer asks for a device sign-in by itself only when `auto_sign_in` is on, the welcome window was never closed, the keychain has no token, and the run is neither `--demo` nor `--headless` (`app.rs:418`, `app.rs:526`).
3. The window code reads the keychain once (`crates/stardust/src/window.rs:129`).
4. `resumed` installs the tray and opens the settings window only when `onboarded` is false (`window.rs:835`).

So a returning user with no session sees no window. The first prompt is a bubble that says "Sign in to see ORBIT items" and "Choose Sign in from the tray menu" (`crates/stardust-core/src/policy.rs:50`). The first check waits for the next schedule slot, and the default interval is 60 minutes (`crates/stardust-core/src/config.rs:191`), so that bubble can be up to an hour away.

### Two sign-in engines

| | Thread engine | Check-timer engine |
| --- | --- | --- |
| Used when | `auto_sign_in` off, and always for `stardust sign-in` (`app.rs:53`) | `auto_sign_in` on, in the running app |
| Entry | `window.rs:505`, `crates/stardust/src/signin.rs:37` | `app.rs:631`, `app.rs:669` |
| Polling | `crates/stardust-core/src/http.rs:157`, blocks a thread | `crates/stardust-core/src/connect.rs:196`, one poll per call |
| `slow_down` | Uses the interval the client reads back, capped at 30 s | Adds 5 s, keeps a larger server value, keeps it for later polls |
| Network error | Ends the sign-in (`http.rs:170`) | Backs off and continues until expiry (`connect.rs:217`) |
| Can it stop early? | No. The thread runs until approval, refusal, or expiry | Only by ending or expiring. No cancel exists |
| Browser | Opens by itself (`signin.rs:48`) | Opens by itself (`app.rs:652`) |

The check-timer engine is the better base. It does no sleeping, keeps network work on one thread, and already has tests against a local server.

### Signed-in state and sign-out

- The token record lives in the OS keychain (`crates/stardust/src/keychain.rs:56`). An access token lasts at most 5 minutes (`crates/stardust-core/src/token.rs:19`). A record stays usable while its refresh token can still be used (`token.rs:138`).
- `current_access` refreshes when it can, and clears the record when ORBIT refuses (`crates/stardust-core/src/session.rs:96`).
- Sign-out is in the settings window (`window.rs:419`) and the CLI (`signin.rs:64`). It deletes the keychain copy. The refresh token is revoked on ORBIT only with `auto_sign_in` on (`window.rs:420`, `signin.rs:65`).

## 2. Gaps

| # | Gap | Evidence |
| --- | --- | --- |
| 1 | No sign-in window at launch for a returning user | `window.rs:841` |
| 2 | The user code is one small settings line, not a large readout | `crates/stardust-core/src/settings.rs:186` |
| 3 | No control to open the ORBIT page. The browser opens once, by itself. If it opens the wrong browser, nothing brings the page back | `signin.rs:48`, `app.rs:652` |
| 4 | No way to copy the code | No clipboard crate in `Cargo.lock` |
| 5 | No countdown. The lifetime is known only inside `DeviceFlow` | `connect.rs:163` |
| 6 | No cancel. Closing the settings window leaves the sign-in polling | `window.rs:326` |
| 7 | Denied, expired, server off, offline, and keychain failure all become one message: "Sign-in did not finish. Try again" | `app.rs:710`, `window.rs:522`, `crates/stardust/src/inbox.rs:22` |
| 8 | The tray offers "Sign in..." even when signed in, has no Sign out, and shows the connection line only with `auto_sign_in` on | `crates/stardust/src/tray.rs:103`, `tray.rs:25` |
| 9 | The bitmap font draws `/`, `?`, `=`, `_`, and `>` as hollow boxes, so a URL is unreadable | `crates/stardust/src/font.rs:250` |
| 10 | A check during a sign-in can raise the "Sign in" bubble on top of the sign-in | `app.rs:868` |
| 11 | After a connection timeout the next poll can come at the same rate, because the backoff floor is the interval | `connect.rs:219`, pinned by the test at `connect.rs:472` |
| 12 | A browser that fails to open is silent or goes only to stderr | `signin.rs:49`, `app.rs:653` |
| 13 | Asking for a code can take up to 10 seconds with nothing on screen | `window.rs:517`, `app.rs:642`, `http.rs:39` |
| 14 | A `stardust sign-in` in a terminal during a run is not noticed. The tray then starts a second, unneeded sign-in | `window.rs:694` |
| 15 | Sign-out reports signed out even when deleting the keychain record failed | `window.rs:427` |

### Bugs found on the way

These exist in `main` today, with every flag off. I am reporting them, not fixing them, because the flag-off behaviour has to stay as it is. Each one is its own small fix if the owner wants it.

- **`--demo` can delete the real sign-in.** Sign out in the settings window clears the keychain whatever the mode (`window.rs:427`), and the row still reads Sign out afterwards (`window.rs:434`). `stardust --demo sign-out` does the same: `sign-in` refuses `--demo` (`crates/stardust/src/cli.rs:243`) and `sign-out` does not (`cli.rs:251`). This is the most serious one, because demo mode is described as making no use of sign-in.
- **The focus marker is a hollow box.** `>` (`settings.rs:204`) has no glyph (`font.rs:250`), although the README says rows are marked with it (`README.md:67`). The existing test checks the string, not the drawing (`settings.rs:450`).
- **With no display and every flag off, tray "Sign in..." does nothing** (`app.rs:556`, `crates/stardust/src/actions.rs:37`). This one comes from the audit's reading, and the no-display tray path was not run.
- **`stardust feature` during a run is lost.** See "Flag" in section 5.

### Limits outside this repository

- **Session length.** A comment says the desktop API contract has no refresh grant yet (`crates/stardust-core/src/api.rs:26`). If ORBIT sends no refresh token, a sign-in lasts at most 5 minutes. The check right after sign-in works, the next scheduled one finds no token, and the next launch shows the window again. The window cannot fix this. I did not read the rust-orbit contract in this run, so its current state needs checking.
- **Revoke route.** `POST /api/pqc/v1/account/device/revoke` is not in that contract either, by the comment at `api.rs:34`. The client already treats a 404 as "not supported".
- **Who is signed in.** `GET /api/pqc/v1/desktop/status` returns an `organization_id` and no name or email (`api.rs:93`). The window can say which host you are signed in to, not which account.

## 3. Reference notes

**Read for this design** (fetched 2026-11-05):

- [GitHub device flow](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps): the app shows the code and sends the user to a fixed page, with no automatic browser open. `slow_down` adds 5 seconds. An expired code means requesting a new one.
- [Microsoft Entra device code](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code): the values are displayed and the user goes to a browser. A refusal and an expired code each stop polling.
- [AWS CLI `sso login`](https://docs.aws.amazon.com/cli/latest/reference/sso/login.html): opens the page by default, and `--no-browser` turns that off.

The fetched text of those pages had no copy step, countdown, or phishing warning, so those parts of the target come from the lane brief and from RFC 8628 as recalled below.

**Not read.** Fetches of RFC 8628, RFC 7009, the `gh auth login` manual, and the arboard pages were refused in this run, and so was web search. The RFC items below are from memory. Check them against the text before relying on the section numbers.

- RFC 8628 section 3.3: show the user code and the verification URI, and instruct the user.
- Section 3.3.1: when using `verification_uri_complete`, still show the user code so the user can compare it.
- Section 3.5: wait the interval between polls, default 5 seconds. `slow_down` adds 5 seconds for this and all later polls. After a connection timeout the client must poll less often, and doubling is recommended. `access_denied` and `expired_token` end the flow.
- Section 5.4, remote phishing: an attacker can start a sign-in and persuade someone to approve the code. The instructions should make clear what is being approved.
- RFC 7009: the client discards its copy whatever the outcome of revocation.

## 4. Target UX

### When the window appears

| Situation | Flag on | Flag off |
| --- | --- | --- |
| Launch, no usable session | Sign-in window opens, sign-in starts | As today |
| Launch, usable session (live token, or one that can be refreshed) | No window | As today |
| Launch, keychain cannot be read | Window opens at "Keychain not available" | As today |
| Tray "Sign in...", or settings Account row, signed out | Sign-in window | As today |
| Token lapses while running | No window. Tray and bubble say signed out | As today |
| `--demo`, `--headless` | No window | As today |
| No display | No automatic sign-in from this flag. A tray click starts one, and the terminal prints the code and address (`app.rs:268`) | As today |

A window does not appear by itself while the app is running, so it never interrupts work. The first-run settings window opens after the sign-in window closes, so two windows never compete at launch.

### Waiting state

```text
+------------------------------------------------------+
|  Sign in to ORBIT                                    |
|                                                      |
|  Enter this code on the ORBIT account page:          |
|                                                      |
|        A B C D - E F G H        (drawn large)        |
|                                                      |
|  helloorbit.app/rust-preview/pqc/account             |
|  Only enter this code if you started this sign-in.   |
|                                                      |
|  Code expires in 9:42                                |
|  Waiting for approval...                             |
|                                                      |
|  [> Open ORBIT in browser ]                          |
|  [  Copy code             ]                          |
|  [  Cancel                ]                          |
|                                                      |
|  Arrows move. Enter chooses. Esc cancels             |
+------------------------------------------------------+
```

The browser does not open by itself. GitHub and Microsoft work the same way, and a window appearing at login should not also launch a browser. "Open ORBIT in browser" is the first focused row, so Enter is enough.

The address line is the plain `verification_uri` without its scheme, so it can be typed on another device. It is never `verification_uri_complete`, which also carries the code. Today `DeviceFlow` keeps only one of the two (`connect.rs:165`), so it needs a second accessor, held to the same https and same-host check. That check does not limit the path (`api.rs:379`), so the address must also be printable ASCII. It is left out when either check fails.

Letters are drawn at a fixed width and cut off at the column limit (`font.rs:99`). The window is 52 columns wide, so all wording has to fit, and longer messages take two lines. An address too long for one line wraps. It is never cut off silently, because the host is the part the user needs to check.

### States

| State | Shown | Rows |
| --- | --- | --- |
| Requesting | "Asking ORBIT for a code..." | Cancel |
| Waiting | Code, host, countdown | Open, Copy, Cancel |
| Reconnecting | Code, countdown, "Cannot reach ORBIT. Still trying" | Open, Copy, Cancel |
| Signed in | "Signed in to helloorbit.app". Closes after 2 seconds | Close |
| Denied | "ORBIT denied this sign-in" | Try again, Not now |
| Expired | "This code expired" | Get a new code, Not now |
| Offline | "Cannot reach ORBIT" | Try again, Not now |
| Unavailable | "Sign-in is not available on this server" | Try again, Not now |
| Keychain failed | "ORBIT approved, but the system keychain did not save the sign-in" | Try again, Not now |
| Keychain not available | "Stardust could not read the system keychain. Unlock it and try again" | Try again, Not now |

- Closing the window, Esc, and Cancel all stop polling. The app keeps running in the tray.
- "Try again" and "Get a new code" always ask for a new code.
- If the browser does not open, the window says "Could not open a browser. Type the address above" and stays on the same code.
- On a plain-http local server there is no allowed page to open (`crates/stardust-core/src/api.rs:379`). The Open row is left out, and the window says to open the ORBIT account page.
- The window moves itself to Expired when the countdown reaches zero. It does not wait for the check timer, which can be busy for a long time (see section 5).

### Accessibility

The window follows the settings model: every row works from the keyboard, focus shows as a text marker and a dark band, and colours come from the existing pairs in `crates/stardust-core/src/a11y.rs`. A screen reader can read the title but not the drawn rows (`README.md:69`), so the title carries the state and the code, for example `Stardust sign-in - Enter code ABCD-EFGH`. The user code is not a secret (`inbox.rs:17`). The countdown stays out of the title, so assistive technology is not interrupted every second.

### Account status and sign-out

With the flag on:

- The tray shows the existing connection line (`ORBIT: Connected`, `Connecting...`, `Signed out`, `Offline`).
- The tray shows "Sign in..." when signed out and "Sign out" when signed in.
- The settings Account row opens the sign-in window instead of starting a sign-in inside settings.
- Sign-out from either place, and from the CLI, revokes the refresh token through the existing path.
- "Sign in..." reads the keychain first, so a sign-in made in a terminal shows as signed in rather than starting another one.
- A sign-out that cannot delete the keychain record says so and stays signed in.
- `--demo` shows no Sign out in the tray, and under this flag its settings row does not touch the keychain.

## 5. Implementation design

### Flag

`Features.launch_sign_in_window`, `Feature::LaunchSignInWindow`, CLI name `launch-sign-in-window`. Same pattern as the existing flags (`config.rs:78`), and off by default.

`Features` denies unknown fields, so an older build cannot read a config that has this key set. A file with all features off omits the section (`config.rs:173`), so it stays readable by older builds.

Quit Stardust before changing the flag. The running app keeps the config in memory (`app.rs:216`) and writes the whole file back after each check (`app.rs:924`), so a `stardust feature` command during a run is overwritten. All flags behave this way today. The README should say so, or the first test of this flag will look like it failed.

### With the flag off

With the flag off, every new branch is skipped, and:

- The config file is written byte for byte as before.
- The thread engine, the `auto_sign_in` engine, the tray menu, and the settings model behave as before.
- The font table `glyph()` is not edited. New characters go in a separate lookup used only by the new window, because adding `>` to the shared table would change how the settings window looks with the flag off.
- `DeviceFlow`'s existing backoff stays as it is for `auto_sign_in` users. The stricter floor for gap 11 applies only under the new flag.
- Shared functions take the flag explicitly where their behaviour differs: the browser open in `start_flow` (`app.rs:652`), the bubble in `soft_signal` (`app.rs:1055`), and the address in `print_sign_in` (`app.rs:1166`). `auto_sign_in` users with the new flag off keep today's output.

A few outputs do change with the flag off, because the new flag has to be discoverable: `stardust feature list` prints one more line, and `--help` and the unknown-feature error (`crates/stardust/src/cli.rs:572`) name it. The existing flags did the same. The binary also grows if a clipboard crate is linked.

### Code

| Part | Where | Change |
| --- | --- | --- |
| Window model | `crates/stardust-core/src/signin_window.rs` (new) | States, rows, keys, clicks, wording, title, countdown text. No display, so it is fully unit-testable, like `settings.rs` |
| Launch rule | same file | `opens_at_launch(flag, demo, session)`, a pure function with a truth-table test |
| Flow | `connect.rs` | Add `remaining(now)`. Add a backoff option used only under the flag |
| Check timer | `app.rs` | With the flag: no automatic browser, send the new updates, handle cancel, keep the "Sign in" bubble away during a sign-in |
| Messages | `crates/stardust/src/inbox.rs` | New `SignInUpdate` variants carrying only the user code, the allowed URL, the lifetime, and the end reason. Never a device code or token |
| Requests | `crates/stardust/src/live.rs` | `request_cancel` and `take_cancel`, same shape as `request_connect` |
| Window | `crates/stardust/src/window.rs` | Open, paint, and handle input for a third window kind. Wake once a second only while the window shows a countdown |
| Tray | `tray.rs`, `crates/stardust-core/src/text.rs` | Status line and the Sign in or Sign out item under the flag |
| Sign-out | `window.rs`, `signin.rs` | Share one path, and revoke with either flag on |
| Font | `font.rs` | Extra glyph lookup for the new window only. Large code uses the existing `Pen.scale` |
| Docs | `README.md`, `PRIVACY.md`, `crates/stardust/src/cli.rs` | Flag description. No new network call is added |

There is one event loop for the process (`window.rs:1`), so the window is another surface in `PopupApp`, not a new loop. Polling stays on the check timer, so the window's only work is drawing. `about_to_wait` already sets `WaitUntil` (`window.rs:1012`). It gains one more deadline only while the countdown is on screen, and idle behaviour is unchanged once the window closes.

Polling after success needs no new code. `SignInUpdate::Done` already asks for an immediate check (`window.rs:541`), and the schedule continues from there.

### Details that are easy to get wrong

- **Gates to widen.** These `auto_sign_in` checks become `auto_sign_in || launch_sign_in_window`, which is unchanged with the new flag off: `app.rs:303`, `app.rs:361`, `app.rs:556`, `window.rs:420`, `window.rs:492`, `window.rs:549`, `tray.rs:25`, `crates/stardust-core/src/text.rs:51`, and `signin.rs:65`. Missing one would leave a route dead with only the new flag on.
- **Cancel and reopen.** `request_cancel` also clears a waiting connect request, and `request_connect` leaves a waiting cancel in place. The check timer handles cancel first. So open-then-cancel starts nothing, and cancel-then-open ends the old code and asks for a new one. Cancel must drop the flow, set the phase to `Idle`, and push `UiMsg::Pet`. If the phase stayed `Waiting`, the tray would keep showing the old code, and `window.rs:494` would adopt a code nobody is polling.
- **Stale updates.** Asking for a code can block the check timer for up to 10 seconds (`crates/stardust-core/src/http.rs:39`), so an answer can arrive after a cancel. The window numbers each attempt in `LivePet`. The timer reads the number when it starts a flow and stamps every update with it, and the window ignores older numbers. The exception is `Done`: the token is already saved by then, so the app must show it as signed in.
- **A busy check timer.** A check makes blocking calls, and each can take 10 seconds (`http.rs:39`), and webhook posts follow inline (`app.rs:512`). So Cancel may take a while to reach the timer, and approval may show late. The window closes at once on Cancel, and it does not depend on the timer for its countdown. Quit can already wait on `scheduler.join()` (`app.rs:375`), and this design does not change that.
- **Sign-out during a sign-in.** `take_sign_out` (`app.rs:580`) does not touch the flow, so a flow still polling could save a token after the user signed out. Under the flag, sign-out also drops the flow, resets the phase, and closes the window. With the flag on, the settings stage never holds `Waiting`, because the Account row would then do nothing (`settings.rs:310`).
- **Approved but not saved.** `keychain.rs:104` reads "no keychain service" as "no record", so a Linux machine with no secret service looks signed out at launch, and the failure appears only when saving (`keychain.rs:16`). The window then shows Keychain failed. The token from that sign-in is dropped, so if it carried a refresh token, the timer revokes that first (`app.rs:702`).
- **Stale signed-in state.** `live.signed_in` changes only on a check, sign-in, or sign-out (`app.rs:933`), but a token with no refresh token lapses after 5 minutes, so the tray could offer "Sign out" for up to an hour. Re-reading the keychain on every tray refresh would be intrusive. Instead `LivePet` keeps the non-secret time the record stops being usable, set when a token is saved or found, and the tray compares it with the clock.
- **Keys.** The settings handler does not ignore key repeat (`window.rs:373`). The new window ignores repeats, and ignores keys for the first half second after opening, so a held or stray Enter at login does not open a browser or cancel a new attempt. After a state change, focus goes to the first row, never to Cancel. Everything also works with the mouse, because some desktops refuse to give a new window focus.
- **First-run settings.** `open_settings` returns early when settings is already open (`window.rs:288`). If the user opens Settings from the tray while the sign-in window is up, the deferred welcome would be lost and `onboarded` would never be set (`window.rs:330`). The app keeps a pending flag, and marks the settings window as first-run when it applies.
- **Reconnecting and sign-out errors.** Reconnecting needs no new message, since a phase change already pushes `UiMsg::Pet` (`app.rs:719`), so the `Pet` handler also redraws this window. A tray sign-out that fails reports through a bubble, because no window may be open.
- **Reading the session at launch.** `signin::signed_in` treats every keychain error as "no session" (`signin.rs:99`). The launch rule needs three answers: usable, none, or unreadable. `TokenError::Keychain` is unreadable. `TokenError::Corrupt` counts as none, because a new sign-in overwrites the record.
- **Countdown wake-ups.** Each second redraws only this window. It must not call `refresh_tray`, which rebuilds the whole menu (`tray.rs:73`) and could disturb an open menu.
- **Opening the browser.** Use `open::that_detached`, as the check timer does (`app.rs:653`), not `open::that` (`signin.rs:49`), which may block. A click must never hold up the window.
- **Sign-in against a running check.** The flow and the checks share one thread (`app.rs:434`), and approval resets the observed check state (`app.rs:698`). So a check cannot be in flight when approval lands. The thread engine has no such protection, which is another reason not to use it here.
- **Window size.** The window is sized for its tallest state, so it does not jump between states. The large code takes up two model lines, so mapping a click to a row (`window.rs:1139`) keeps working.
- **Both flags on.** `first_run_connect` (`app.rs:526`) returns false with the new flag on, so the window is the only thing that starts a sign-in at launch. Otherwise an unnumbered flow could start before the window exists, and the window would ignore its code. With the new flag off it is unchanged.
- **No display.** The terminal fallback prints the code today but not the address (`app.rs:1166`). With no automatic browser it has to print the address too.

### Security

- The device code, access token, and refresh token never reach the UI, the messages, or the logs. Their `Debug` output is already redacted.
- `user_code` is checked against a fixed alphabet before it can be drawn (`http.rs:290`).
- The only page the window opens is `verification_url`, which must be https on the configured host (`api.rs:379`), and only on a click. `stardust sign-in` in a terminal still opens it by itself.
- The window shows the host and a one-line warning, for the phishing case in section 3.
- Copy places only the user code on the clipboard.
- A cancelled sign-in stops polling on this computer. The code stays valid on ORBIT until it expires, because no route cancels it. A poll already on its way when the cancel is handled can still finish the sign-in, and the tray shows it. An approval no poll has collected yet is not picked up.

### Clipboard

Copy needs a new dependency, because neither `winit` nor the existing crates provide one (none in `Cargo.lock`). The first choice is `arboard` with default features off, created on the first click and dropped when the window closes.

I could not read its documentation in this run. Before adding it, check: the license against `deny.toml`, the transitive crates with `cargo tree`, Wayland support, how long clipboard contents survive on Linux, and the idle memory limits in `scripts/measure-idle-lane.sh`. The README's dependency tables need updating too. If it does not pass, the fallback is to leave Copy out and keep the code large and readable.

## 6. Everything a user can connect

Owners: **this lane** is `feat/launch-signin-window-1105`. **sdSignals** is `feat/signal-delivery-ux-1105`, which is building the integrations panel, consent, and delivery state separately. This table describes `6bcf9a7` and does not include that lane's work, which I have not seen.

| Connection | How it connects today | Secret | Test or disconnect | Window UI | Flag | Owner of the gap |
| --- | --- | --- | --- | --- | --- | --- |
| ORBIT account | `stardust sign-in`, tray, settings | Keychain | Check now; `sign-out`, settings | Settings row | Optional `auto-sign-in` | This lane |
| Slack webhook | `integration url` | Keychain | `integration test` | None | Optional `routing` | sdSignals |
| Teams webhook | `integration url` | Keychain | `integration test` | None | Optional `routing` | sdSignals |
| JSON webhook | `integration url`, with a signing secret | Keychain | `integration test` is refused for this kind (`crates/stardust/src/routing.rs:266`) | None | None | sdSignals |
| Discord | `integration url` | Keychain | `integration test` | None | `routing` required | sdSignals |
| PagerDuty | `integration key`, optional `url` | Keychain | `integration test` | None | `routing` required | sdSignals |
| ORBIT MCP | `integration url`, `integration token` | Keychain; the endpoint is in config | `integration connect` | None | None | sdSignals |
| Slack MCP | `integration oauth`, or `token` | Keychain | `integration connect` | None | None | sdSignals |
| OS keychain | Automatic | Is the store | None | None | None | This lane, for failure states during sign-in |
| OS notifications | Automatic | None | None | None | `routing` narrows them | Neither |
| Do-not-disturb | Read-only probe | None | None | None | `routing` only | Neither |
| Start at login | `autostart`, settings | None | `autostart status`, `off` | Settings row | None | Neither |

**AWS and other cloud accounts.** Stardust has no such connection. A search of the repository for AWS, Amazon, S3, IAM, STS, Bedrock, Cognito, GCP, Google, Cloudflare, Lambda, SSO, SAML, and Azure found no connection code, and `Cargo.lock` has no such SDK. The only code hits are a blocked metadata host (`crates/stardust-core/src/outbound.rs:547`) and the Teams webhook host suffix (`crates/stardust-core/src/sink.rs:29`). The owner's "AWS connection" example therefore belongs to ORBIT, where the checks run. Stardust reads results from ORBIT and holds no cloud credentials, and this design keeps it that way. If Stardust should point people to ORBIT's AWS setup, that is a link to an ORBIT page, and a separate decision.

### Gaps for each owner

**This lane:** gaps 1 to 15 in section 2, account status and sign-out in the tray, and revoking on sign-out with the new flag on.

**sdSignals, passed on here because this audit found them:**

- All integration setup is CLI only. Settings has no integration row (`crates/stardust-core/src/settings.rs:15`).
- There is no way to remove an integration or its secrets. `disable` keeps them, and the only integration secret ever deleted is the temporary sign-in verifier (`crates/stardust/src/oauth.rs:40`).
- `integration url` saves secrets while the integration is off (`crates/stardust/src/integrations_cmd.rs:187`). `PRIVACY.md:10` says they are held "only if you turn an integration on".
- `enable` does not check that a destination is saved (`crates/stardust-core/src/config.rs:350`).
- Delivery results reach stderr only (`crates/stardust/src/deliver.rs:24`), so a user cannot see a failed post.
- `README.md` and `PRIVACY.md` never mention Discord, PagerDuty, routing keys, or the revoke call, so `PRIVACY.md` does not list every network call. The revoke line is shared: this lane adds it if the flag ships.

**Neither lane:** notifications have no off switch, and a failed do-not-disturb probe reads as off (`crates/stardust/src/dnd.rs:18`).

## 7. Test plan

Local only. No test contacts a real host, and tests never touch the real keychain.

- **Model:** every state's lines and rows, keyboard reach, click mapping, countdown text, ASCII and copy-check wording, every line fits 52 columns, long addresses wrap, title text, no secrets in `Debug`.
- **Requests:** open-then-cancel starts nothing, cancel-then-open starts one new flow, cancel resets the phase, sign-out during a sign-in drops the flow, and updates from an older attempt are ignored.
- **Gates:** each widened check, with only the new flag on, with only `auto_sign_in` on, and with both.
- **Launch rule:** truth table over flag, demo, and session states.
- **Flow against `TestServer`** (`crates/stardust-core/src/testserver.rs`) with `MemoryTokenStore`: pending, `slow_down`, approval and storage, denial, expiry by server and by clock, 404, offline then recovery, cancel stops further polls, and backoff with the flag on and off.
- **Flag off:** config output unchanged, tray text unchanged, settings lines unchanged, font table unchanged.
- **End to end:** `scripts/mock_orbit.rs` on loopback with `STARDUST_SERVER_URL`. On loopback no browser page is allowed, so nothing external opens. Add scripted denial and expiry modes to the mock.
- **Idle:** `scripts/measure-idle-lane.sh` with the flag on and off.
- **CI:** `scripts/ci.sh`, and `cargo check` on Windows and macOS through the existing workflow.

Not covered by automated tests: drawing and tray behaviour on real desktops. These need a manual pass on Linux, macOS, and Windows before the flag is turned on for anyone.

## 8. Decisions for the owner

1. **Refresh tokens.** Does ORBIT send them yet? Without them this window appears on nearly every launch and a session lasts 5 minutes. This matters more than any UI detail here. Recommendation: confirm before turning the flag on.
2. **Start at login.** A launch at login cannot be told apart from a manual launch, so the window appears at login too when signed out. Recommendation: accept this for the preview, because "Not now" is one key away. A later change could pass an argument from the autostart entry.
3. **Clipboard crate.** Accept a new dependency for Copy, subject to the checks in section 5. Recommendation: yes.
4. **Sign-out.** Immediate, with no confirmation, matching the settings row. Recommendation: keep.
5. **The `--demo` sign-out bug** in section 2. It affects builds with every flag off, so fixing it is outside this flag. Recommendation: fix it separately and soon.

## 9. As built

Where the build differs from sections 1 to 8, and what those sections left open.

- **Attempts and cancel are one lock.** Section 5 describes `request_cancel` and `take_cancel` as separate calls. Built that way, a "Try again" landing between the two takes would start a flow and then drop it, leaving a code on screen that nobody polls. So `LivePet::request_new_sign_in` sets the attempt, the cancel, and the connect together, and `take_flow_requests` takes the cancel and the connect together.
- **Input guard.** Section 5 asks for keys to wait half a second after the window opens. The guard also restarts on each state change and covers clicks, so a key meant for the last state, or the second half of a double click on Try again, cannot hit the first row of the next state.
- **Letters.** The extended table also has lowercase, with an eighth row so `g`, `j`, `p`, `q`, and `y` hang below the line. Without lowercase an address reads in capitals, and a path can be case-sensitive. Without the eighth row `g` reads as `9`. The shared table is unchanged, so the other windows look as before.
- **An address that cannot be drawn.** Over three lines, or rejected by the checks in section 4, it is replaced by the configured host and a one-line hint, which names the Open row only when there is one. So a code is never on screen without a host, whatever the server sends. Section 5 says the window shows the host; the host is only ever wrapped, and the window grows if a very long one needs it.
- **Rows stay put.** The note line is always reserved in the waiting state, so a note appearing does not move a row under the pointer.
- **Requesting has nothing focused.** Section 4 gives this state only a Cancel row, and section 5 says focus never lands on Cancel. Asking for a code can take ten seconds, so a second Enter must not end the attempt. Enter does nothing here, and Esc and a click still cancel.
- **Closing always cancels.** There is one way to close the window, and it always asks the timer to drop the flow, so no flow can outlive its window. An approval landing while a new attempt waits also drops that attempt, and a code or ending that arrives after "Signed in" is ignored.
- **Sign out then Quit.** With this flag on, the timer finishes a waiting sign-out before it exits, so the revoke and the second delete are not lost. With only `auto_sign_in` on, the old order stays, which can lose them. That is a bug in `main` to fix separately.
- **`--demo sign-out`.** With this flag on it is refused, because it would otherwise delete a real record and now also revoke it on ORBIT. With this flag off it stays as it was, which is the bug in section 2.
- **Flag-off output.** `stardust feature NAME on|off` prints the new line as well as `feature list`, since they share one loop. Section 5 names only `list`.
- **`arboard` checks.** `cargo deny` passes for bans, licenses, sources, and advisories, the advisories against the local copy of the database, with no fetch. It adds four crates on Linux and two more on Windows, listed in `README.md`, and does not change `x11rb`'s features. With default features off it copies text only. On Linux it reaches the clipboard through X11, so a Wayland session with no X11 bridge shows "Could not copy". The clipboard is created only on a click.
- **Idle memory.** Measured against a release build of `6bcf9a7`, read from `/proc/PID/status`, using the limit in `scripts/measure-idle-lane.sh` (1024 KiB or 10 %). This machine has no display, so the runs were `--headless` and the tray under `dbus-run-session` with no window, on a loopback server, signed out. That script itself was not run. Each run has one reading rather than a median of five; repeat readings of a running process were identical.

| Run | Base RSS | Branch, flag off | Branch, flag on |
| --- | --- | --- | --- |
| Headless | 7140 KiB | 7076 KiB | 7124 KiB |
| Tray, first round | 10100 KiB | 10596 KiB (+4.9 %) | 10784 KiB (+6.8 %) |
| Tray, second round, launch order reversed | 10536 KiB | 10508 KiB | 10592 KiB |

  Base alone moved 436 KiB between tray rounds. Private memory (`RssAnon`) stayed between 1864 and 1880 KiB in all tray runs, so what moves is file-backed pages of the binary. All runs are within the limit. An idle run with the window open has not been measured, nor has the app after a first Copy.
- **Testing the mock.** `scripts/mock_orbit.rs` is compiled into the test build of the `stardust` crate, and two tests drive the real check-timer functions against it. So it now follows `cargo fmt`. These tests stop short of the window, because this machine has no display.

Still not exercised by anything automated: the winit event handlers for this window, the tray menu on a real desktop, and the real keychain. Drawing was checked by eye from PNGs written by the ignored `preview_sign_in_window` test, on Linux only. The manual pass in section 7 is still required before the flag is turned on for anyone.
