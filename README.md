# uwu-vibe

Turn a Wooting UwU RGB into a modal controller for T3 Code and Codex, with
per-target keymaps and live agent-status lighting.

This build is Mac-first. It reads the UwU's raw analog HID reports, observes T3
through its authenticated read-only API (with a SQLite fallback), observes
Codex through `codex app-server`, drives the UwU RGB interface, and invokes the
active app's existing commands.

## Targets and layers

The controller has two independent modes: T3 and Codex. Each target keeps its
own active layer, actions, layer colors, and status palette. Switching target
flashes the target accent across the device; the four top-edge LEDs retain that
accent afterward.

Tap one of the three small buttons to select its persistent layer. Hold a
button for 350 ms to arm its temporary hold layer, then press an HE key.
Releasing the button returns from the hold layer. A target switch triggered
from a hold layer is applied only after release, so the gesture cannot leak
into the newly selected target.

Double-tap a button to invoke its target-specific `double_tap_action`. By
default, double-tapping the middle button cycles between T3 and Codex while the
left and right double-taps are unassigned. The middle button's single tap is
committed after the 250 ms double-tap window expires; buttons without a
double-tap binding still select their layers immediately.

HE keys can also have optional hold and double-tap actions, including
per-key analog actuation and release thresholds. These advanced HE gestures are
unassigned in both default target maps, so the default tap actions still fire
immediately on actuation.

An HE key can instead use peak-depth selection. In this mode no action fires on
the way down. The controller records the deepest travel, waits for upward
movement, selects only the deepest matching activation point, and latches until
the key fully releases.

Default persistent layers:

| Button / layer | Left HE key | Middle HE key | Right HE key |
|---|---|---|---|
| 1 — Agents | Thread 1 | Thread 2 | Thread 3 |
| 2 — Chat | New chat | Command palette | Diff panel |
| 3 — Tools | Terminal | Preview | Model picker |

Default hold layers:

| Hold button | Left HE key | Middle HE key | Right HE key |
|---|---|---|---|
| 1 — More agents | Thread 4 | Thread 5 | Thread 6 |
| 2 — Navigate | Previous thread | Next thread | New local chat |
| 3 — Workspace | Sidebar | Right panel | Cycle target |

`target.next` (also spelled `target.cycle`), `target.previous`,
`target.select.t3`, and `target.select.codex` can be assigned anywhere in a
target keymap. `default_target` and `target_order` control startup and cycling.

On the Agents layer, the three HE LEDs use the active target's status palette:

- blue: running or starting
- orange: waiting for approval
- yellow: waiting for input
- green: completed
- red: failed
- the target's idle/unknown color: idle or no thread

T3 thread keys use its numbered keyboard shortcuts. Codex thread keys read the
six most recent local threads from app-server and open the selected technical
thread ID through the desktop app's `codex://threads/<id>` deep link.

## Build and run

Requirements: macOS, Rust, a Wooting UwU RGB, and either T3 Code, Codex, or
both. Codex support expects the `codex` CLI to be on `PATH`.

```sh
git clone https://github.com/devteapot/uwu-vibe.git
cd uwu-vibe
cargo build --release
```

Create a dedicated Wootility profile and remove the keyboard binding from all
three HE keys and all three top buttons. Save that profile to the device, then
quit Wootility so it does not reclaim the RGB interface.

Useful checks:

```sh
cargo run -- diagnose
cargo run -- diagnose --watch
cargo run -- test-rgb
cargo run -- reset-rgb
cargo run -- state t3
cargo run -- state codex
cargo run -- action chat.new --target codex
cargo run
```

On a normal `cargo run`, the daemon prints the configured keymap for every
enabled target before it starts handling input.

The first shortcut action may prompt for macOS Accessibility permission. Enable
the terminal (or packaged app) that launched `uwu-vibe` under **System Settings →
Privacy & Security → Accessibility**, then restart that application. Codex
thread selection and new-chat actions use desktop deep links and do not require
keystroke permission.

## T3 state setup

Pairing is optional but recommended. Without it, `uwu-vibe` uses the local
read-only SQLite observer.

Create a client pairing link in T3 Code's remote-access settings, then run:

```sh
cargo run -- pair
cargo run -- state t3
```

The pairing credential is exchanged for `orchestration:read` access and stored
in macOS Keychain. It is never written to the config file. In `auto` mode, an
unavailable API falls back to SQLite. Run `cargo run -- unpair` to delete the
saved credential.

For a remote T3 server, set `t3_http_url`. For automation, a token can instead
be supplied through `UWU_VIBE_T3_BEARER_TOKEN` (or the environment variable named
by `t3_bearer_token_env`).

## Codex state setup

The baseline Codex adapter starts a read-only stdio app-server and uses
`thread/list` plus `thread/read` to resolve the six latest local threads. This
provides thread identity and settled state without modifying Codex.

For immediate running, approval, input, and completion transitions across
separate Codex processes, add lifecycle hooks. Every handler can use the same
command; replace `/ABSOLUTE/PATH/uwu-vibe` with the release binary:

```toml
[[hooks.SessionStart]]
[[hooks.SessionStart.hooks]]
type = "command"
command = "/ABSOLUTE/PATH/uwu-vibe codex-hook"
timeout = 5

[[hooks.UserPromptSubmit]]
[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "/ABSOLUTE/PATH/uwu-vibe codex-hook"
timeout = 5

[[hooks.PermissionRequest]]
[[hooks.PermissionRequest.hooks]]
type = "command"
command = "/ABSOLUTE/PATH/uwu-vibe codex-hook"
timeout = 5

[[hooks.PreToolUse]]
matcher = "^request_user_input$"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/ABSOLUTE/PATH/uwu-vibe codex-hook"
timeout = 5

[[hooks.PostToolUse]]
matcher = "^request_user_input$"
[[hooks.PostToolUse.hooks]]
type = "command"
command = "/ABSOLUTE/PATH/uwu-vibe codex-hook"
timeout = 5

[[hooks.Stop]]
[[hooks.Stop.hooks]]
type = "command"
command = "/ABSOLUTE/PATH/uwu-vibe codex-hook"
timeout = 5
```

Put this in the `[hooks]` portion of `~/.codex/config.toml` for all projects,
or in a trusted project's `.codex/config.toml`. Codex requires review of new
command hooks; use `/hooks` to inspect and trust them. See the official
[Codex hooks documentation](https://learn.chatgpt.com/docs/hooks) for lifecycle
and trust details.

## Configuration

Copy `uwu-vibe.example.toml` to `uwu-vibe.toml`, edit it, and run:

```sh
cargo run -- --config uwu-vibe.toml
```

Each `[targets.<id>]` owns an accent, a full status palette, and exactly three
layers. Every layer has three base actions, an optional button
`double_tap_action`, and a three-action button-hold map. Omitted gesture actions
are disabled; writing `"none"` remains accepted for older configurations but
is unnecessary.

`key_gestures` is an array of exactly three entries in left, middle, right HE
order. It may be placed on either a persistent layer or that layer's `.hold`
table. If the field is omitted, all three keys inherit their ordinary tap
behavior and the global analog thresholds.

### Gesture resolution

Gesture recognition only changes timing when the corresponding optional action
is configured:

| HE configuration | Resolution |
|---|---|
| No advanced gesture | The ordinary `actions` entry fires immediately at actuation |
| `hold_action` | A short press fires the ordinary action on release; holding through `key_hold_ms` fires the hold action |
| `double_tap_action` | One tap fires the ordinary action after `double_tap_ms`; two taps fire the double-tap action |
| Hold and double-tap | A sustained press resolves as hold; short presses remain eligible for double-tap |
| `depth_actions` | Peak-depth mode replaces ordinary tap, hold, and double-tap resolution |

For layer buttons, a hold that reaches `combo_hold_ms` takes priority over tap
or double-tap. A button without `double_tap_action` selects its layer
immediately on release. A configured double-tap action delays that button's
single-tap selection until the double-tap window closes.

Depth mode is intentionally exclusive. Configuration validation rejects a key
that combines `depth_actions` with `hold_action` or `double_tap_action`.
Explicit `"none"` values are treated as unset for compatibility.

### Timing and analog settings

| Setting | Default | Valid range | Meaning |
|---|---:|---:|---|
| `actuation_threshold` | `0.42` | `0.0..1.0` | Global HE press point |
| `release_threshold` | `0.18` | `0.0..1.0`, below actuation | Global HE release point |
| `combo_hold_ms` | `350` | `100..5000` | Layer-button hold duration |
| `key_hold_ms` | `350` | `100..5000` | HE-key hold duration |
| `double_tap_ms` | `250` | `100..1000` | Button and HE double-tap window |
| `depth_reversal_hysteresis` | `0.04` | `0.01..0.25` | Upward travel needed to resolve a peak |
| `poll_interval_ms` | `750` | `100` or greater | Target-state refresh interval |
| `brightness` | `0.65` | `0.0..1.0` | Global LED brightness |

Each `key_gestures` entry supports:

| Field | Meaning |
|---|---|
| `hold_action` | Action fired after `key_hold_ms` |
| `double_tap_action` | Action fired by two short presses |
| `depth_actions` | Strictly increasing `{ at, action }` peak-depth bands |
| `depth_fallback_action` | Optional action below the first depth band |
| `actuation_threshold` | Per-key press/tracking-start override |
| `release_threshold` | Per-key release override |

Advanced HE behavior is configured per layer and per key. This example assigns
gestures only to the first HE key; the other two entries inherit the ordinary
tap behavior and global thresholds:

```toml
[[targets.codex.layers]]
name = "Agents"
color = "#10a37f"
actions = ["thread.jump.1", "thread.jump.2", "thread.jump.3"]
key_gestures = [
  { hold_action = "chat.new", double_tap_action = "thread.jump.4", actuation_threshold = 0.70, release_threshold = 0.20 },
  {},
  {}
]

[targets.codex.layers.hold]
name = "More agents"
color = "#48c6a9"
actions = ["thread.jump.4", "thread.jump.5", "thread.jump.6"]
```

Actuation and release values range from `0.0` to `1.0`, and release must remain
below actuation. Each omitted threshold inherits the global
`actuation_threshold` or `release_threshold`.

Peak-depth actions use the same per-key field:

```toml
key_gestures = [
  { depth_actions = [{ at = 0.33, action = "thread.jump.1" }, { at = 0.66, action = "thread.jump.2" }, { at = 0.95, action = "thread.jump.3" }] },
  {},
  {}
]
```

Points must be strictly increasing. The first point becomes the key's
actuation threshold unless `actuation_threshold` is explicitly set.
`depth_fallback_action` can optionally handle a peak below the first point.
That fallback is reachable when an explicit actuation threshold starts
tracking below the first depth point. When depth mode is enabled, it replaces
the key's ordinary tap action. Using approximately `0.95` for bottom-out is
more tolerant of physical sensor variation than requiring exactly `1.0`.

`depth_reversal_hysteresis` controls how far the key must move upward before
the selected depth action fires; its default is `0.04`. If a report jumps
directly to the release point, the same peak is resolved on release. Depth mode
cannot be combined with hold or double-tap actions on the same HE key, and
configuration validation rejects ambiguous combinations.

### Supported actions

All action fields use the same names:

| Action | Behavior |
|---|---|
| `thread.jump.1` through `thread.jump.6` | Select a numbered T3 thread or open the corresponding recent Codex thread |
| `thread.previous`, `thread.next` | Navigate between threads |
| `chat.new`, `chat.newLocal` | Start a chat; both open a new local chat in Codex |
| `commandPalette.toggle` | Open the command palette |
| `diff.toggle` | Toggle the diff/review surface |
| `terminal.toggle` | Toggle the terminal |
| `preview.toggle`, `preview.refresh` | Toggle or refresh preview |
| `modelPicker.toggle` | Open the model picker surface |
| `sidebar.toggle`, `rightPanel.toggle` | Toggle app panels |
| `app.activate` | Bring the target application to the foreground |
| `target.next`, `target.cycle` | Select the next enabled target |
| `target.previous` | Select the previous enabled target |
| `target.select.t3`, `target.select.codex` | Select a specific enabled target |
| `none` | Compatibility no-op; omission is preferred for optional actions |

App actions use the target's current default macOS shortcut mapping. Codex
thread jumps and new-chat actions use `codex://` deep links. Unknown action
names are reported at runtime. The current Codex adapter maps
`modelPicker.toggle` to `Cmd+K` as a best-effort picker/command-palette entry
point.

The included targets are `t3` and `codex`; adding another target has a single
adapter boundary for state and action dispatch rather than requiring changes
throughout the input and RGB loops.

Version 0.3 files with top-level `[[layers]]` still load: those entries replace
only the T3 keymap, while Codex receives its defaults. Use the nested target
shape in the example for new configurations.

### Migrating from t3-uwu

The binary and crate are now named `uwu-vibe`. Existing configuration contents
remain valid when supplied with `--config`. The application also reads the old
`T3_UWU_BEARER_TOKEN`, `devteapot.t3-uwu` Keychain entry, and `t3-uwu` Codex
hook-state directory as fallbacks. New credentials and hook events use the
`uwu-vibe` names.

Update existing Codex hook commands to invoke `uwu-vibe codex-hook`. Running
`uwu-vibe unpair` removes credentials stored under either project name.

The default input positions are:

- HE keys: `r2c1`, `r2c3`, `r2c5`
- layer buttons: `r3c2`, `r3c3`, `r3c4`

RGB control is released on normal exit. If a force-killed process leaves SDK
control active, run `cargo run -- reset-rgb`.

See [NOTICE](NOTICE) for protocol attribution.
