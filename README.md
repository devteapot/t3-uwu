# t3-uwu

Turn a Wooting UwU RGB into a small, layered T3 Code controller with live Codex
status lighting.

This build is Mac-first: it reads the UwU's raw analog HID reports, observes T3
Code through its authenticated read-only API (with a local SQLite compatibility
fallback), drives the UwU's RGB interface, and invokes T3's existing keyboard
commands.

## Default layout

Tap one of the three small buttons to select its persistent layer. Hold a
button for 350 ms to arm its temporary hold layer, then press any of the three
HE keys. Releasing the button returns to the previously selected layer. A long
hold with no HE action behaves like a tap and selects that layer.

The selected layer button is white. When a hold layer arms, the button and HE
keys change color so the mode switch is visible before an action fires.

Persistent layers:

| Button / layer | Left HE key | Middle HE key | Right HE key |
|---|---|---|---|
| 1 — Agents | Thread 1 | Thread 2 | Thread 3 |
| 2 — Chat | New chat | Command palette | Diff panel |
| 3 — Tools | Terminal | Preview | Model picker |

Hold layers:

| Hold button | Left HE key | Middle HE key | Right HE key |
|---|---|---|---|
| 1 — More agents | Thread 4 | Thread 5 | Thread 6 |
| 2 — Navigate | Previous thread | Next thread | New local chat |
| 3 — Workspace | Sidebar | Right panel | Refresh preview |

On the Agents layer, the main key LEDs reflect the first three threads in T3's
current sidebar-v2 ordering: active threads in newest-created order, followed
by the settled tail. This matches the targets of T3's `Cmd+1` through `Cmd+3`
shortcuts when the sidebar is unscoped:

- blue: running or starting
- orange: waiting for approval
- yellow: waiting for input
- green: completed/ready
- red: failed
- dim: no thread/unknown

The mapping exposes 18 actions while keeping the three top buttons useful as
ordinary layer selectors. The HE input path also preserves analog travel, so
dual-stage or press-depth gestures can be added without a hardware change.

## Build and run

Requirements: macOS, Rust, T3 Code, and a Wooting UwU RGB.

```sh
git clone https://github.com/devteapot/t3-uwu.git
cd t3-uwu
```

First create a dedicated profile in Wootility and remove the keyboard binding
from all three HE keys and all three top buttons. Save that profile to the UwU.
The firmware continues exposing their physical state through the analog HID
interface, but they will no longer type their old bindings alongside the bridge
actions. Wootility may warn that unbound keys will not input anything; that is
intentional for this profile.

After saving the profile, quit Wootility before starting `t3-uwu`. Wootility
can reclaim the RGB interface and replace the bridge's status colors even when
the HID writes themselves report success. The daemon prints a warning when it
detects Wootility running.

```sh
cargo build --release
cargo run -- diagnose
cargo run -- test-rgb
cargo run -- reset-rgb
cargo run -- action thread.jump.1
cargo run -- t3-state
cargo run
```

The first action may prompt for macOS Accessibility permission because the
bridge activates T3 Code and sends its configured keyboard shortcut through
System Events. Grant permission to the terminal (or to the packaged app when we
add one) under **System Settings → Privacy & Security → Accessibility**.
The `action` command is the quickest way to verify that permission separately
from the hardware input path.

### Pair with the T3 API

Pairing is optional but recommended. Without it, `t3-uwu` continues to use the
local read-only SQLite observer.

In T3 Code, create a client pairing link under its remote-access settings. Then
run the command below and paste the full link when prompted. Leaving the URL out
of the command keeps its one-time credential out of shell history.

```sh
cargo run -- pair
cargo run -- t3-state
```

The pairing credential is exchanged for `orchestration:read` access only. The
resulting bearer token is saved in macOS Keychain; it is never written to the
config file. `t3-state` prints `State source: T3 API` when the connection is
active. If the credential is revoked or T3 is temporarily unreachable, `auto`
mode falls back to SQLite and the running daemon reports the transition once.
Run `cargo run -- unpair` to delete the locally saved credential.

For a remote T3 server, set `t3_http_url` in the config. For automation, a token
can instead be supplied through `T3_UWU_BEARER_TOKEN` (or the environment
variable named by `t3_bearer_token_env`).

Use `cargo run -- diagnose --watch` to verify matrix positions with concise
press/release events. Add `--raw` to see the full analog travel stream. Press
each HE key and top button; the default UwU RGB firmware layout is:

- HE keys: `r2c1`, `r2c3`, `r2c5`
- layer buttons: `r3c2`, `r3c3`, `r3c4`

If your firmware exposes different positions, copy `t3-uwu.example.toml` to
`t3-uwu.toml`, edit it, and run `cargo run -- --config t3-uwu.toml`.

## Configuration and current limits

Each of the exactly three `[[layers]]` entries requires a base `name`, `color`,
and three `actions`, plus a `[layers.hold]` table with its own `name`, `color`,
and three `actions`. `combo_hold_ms` controls the hold threshold and accepts
values from 100 through 5000 ms. This configuration shape is new in v0.3 and
older custom files must add the required hold tables; see
`t3-uwu.example.toml` for a complete example.

The action names supported in the example config are the T3 commands
implemented by this prototype. They use T3's default macOS shortcuts, so
customized T3 keybindings may need matching changes in `src/actions.rs` for
now.

The default `t3_state_source = "auto"` uses the authenticated shell-snapshot API
after pairing and otherwise opens `t3_database` read-only. Set it to `"api"` to
require API access or `"sqlite"` to force the compatibility backend. The local
API URL is discovered through `t3_runtime`; `t3_http_url` overrides it.

Thread selection and status reproduce the default, unscoped sidebar-v2 order.
If a project scope chip is active in T3, `Cmd+1` through `Cmd+3` refer to that
filtered view while this prototype still observes the global list. A future
native T3 peripheral API should expose the exact visible order, selected
thread, action dispatch, and a push event stream. Until that API exists, the
read-only shell snapshot plus standard T3 shortcuts keep this build useful;
SQLite remains available for older T3 releases and expired credentials.

RGB control takes over while the process runs and restores the onboard Wooting
effect when the process exits normally. During development, use Ctrl-C once and
then run `cargo run -- reset-rgb` if a force-killed process leaves Wooting's RGB
SDK-control flag in place. Ctrl-C, SIGTERM, and terminal hangup all use the
normal cleanup path; SIGKILL and a power loss cannot run process cleanup.

See [NOTICE](NOTICE) for protocol attribution.
