# t3-uwu

Turn a Wooting UwU RGB into a small, layered T3 Code controller with live Codex
status lighting.

This first version is Mac-first and deliberately local: it reads the UwU's raw
analog HID reports, reads T3 Code's local projection database in read-only mode,
drives the UwU's RGB interface, and invokes T3's existing keyboard commands.

## Default layout

The three small buttons select a layer. The selected layer button is white; the
other two show their layer colors dimly.

| Button / layer | Left HE key | Middle HE key | Right HE key |
|---|---|---|---|
| 1 — Agents | Thread 1 | Thread 2 | Thread 3 |
| 2 — Chat | New chat | Command palette | Diff panel |
| 3 — Tools | Terminal | Preview | Model picker |

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

The mapping gives nine actions immediately. The HE input path also preserves
analog travel, so dual-stage or press-depth gestures can be added without a
hardware change.

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

```sh
cargo build --release
cargo run -- diagnose
cargo run -- test-rgb
cargo run -- action thread.jump.1
cargo run
```

The first action may prompt for macOS Accessibility permission because the
bridge activates T3 Code and sends its configured keyboard shortcut through
System Events. Grant permission to the terminal (or to the packaged app when we
add one) under **System Settings → Privacy & Security → Accessibility**.
The `action` command is the quickest way to verify that permission separately
from the hardware input path.

Use `cargo run -- diagnose --watch` to verify matrix positions with concise
press/release events. Add `--raw` to see the full analog travel stream. Press
each HE key and top button; the default UwU RGB firmware layout is:

- HE keys: `r2c1`, `r2c3`, `r2c5`
- layer buttons: `r3c2`, `r3c3`, `r3c4`

If your firmware exposes different positions, copy `t3-uwu.example.toml` to
`t3-uwu.toml`, edit it, and run `cargo run -- --config t3-uwu.toml`.

## Configuration and current limits

The action names supported in the example config are the T3 commands implemented
by this prototype. They use T3's default macOS shortcuts, so customized T3
keybindings may need matching changes in `src/actions.rs` for now.

Thread selection and status reproduce the default, unscoped sidebar-v2 order.
If a project scope chip is active in T3, `Cmd+1` through `Cmd+3` refer to that
filtered view while this prototype still observes the global list. A future
native T3 peripheral API should expose the exact visible order, selected
thread, action dispatch, and a push event stream. Until that API exists, the
read-only SQLite observer plus standard T3 shortcuts keep this build useful
without modifying or reverse-engineering T3's authenticated local RPC.

RGB control takes over while the process runs and restores the onboard Wooting
effect when the process exits normally. During development, use Ctrl-C once and
then run `cargo run -- test-rgb` if a force-killed process leaves an RGB frame in
place.

See [NOTICE](NOTICE) for protocol attribution.
