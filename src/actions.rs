use std::process::Command;

use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, Debug)]
struct Shortcut {
    key: &'static str,
    command: bool,
    shift: bool,
    option: bool,
    control: bool,
}

impl Shortcut {
    const fn cmd(key: &'static str) -> Self {
        Self {
            key,
            command: true,
            shift: false,
            option: false,
            control: false,
        }
    }
    const fn cmd_shift(key: &'static str) -> Self {
        Self {
            key,
            command: true,
            shift: true,
            option: false,
            control: false,
        }
    }
    const fn cmd_option(key: &'static str) -> Self {
        Self {
            key,
            command: true,
            shift: false,
            option: true,
            control: false,
        }
    }
}

pub fn run(action: &str, app_name_contains: &str) -> Result<()> {
    let shortcut = match action {
        "thread.jump.1" => Shortcut::cmd("1"),
        "thread.jump.2" => Shortcut::cmd("2"),
        "thread.jump.3" => Shortcut::cmd("3"),
        "thread.jump.4" => Shortcut::cmd("4"),
        "thread.jump.5" => Shortcut::cmd("5"),
        "thread.jump.6" => Shortcut::cmd("6"),
        "thread.previous" => Shortcut::cmd_shift("["),
        "thread.next" => Shortcut::cmd_shift("]"),
        "chat.new" => Shortcut::cmd("n"),
        "chat.newLocal" => Shortcut::cmd_shift("n"),
        "commandPalette.toggle" => Shortcut::cmd("k"),
        "diff.toggle" => Shortcut::cmd("d"),
        "terminal.toggle" => Shortcut::cmd("j"),
        "preview.toggle" => Shortcut::cmd_shift("j"),
        "preview.refresh" => Shortcut::cmd("r"),
        "modelPicker.toggle" => Shortcut::cmd_shift("m"),
        "sidebar.toggle" => Shortcut::cmd("b"),
        "rightPanel.toggle" => Shortcut::cmd_option("b"),
        "none" => return Ok(()),
        other => bail!("unknown action {other:?}"),
    };

    let modifiers = [
        shortcut.command.then_some("command down"),
        shortcut.shift.then_some("shift down"),
        shortcut.option.then_some("option down"),
        shortcut.control.then_some("control down"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let escaped_name = app_name_contains.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_key = shortcut.key.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "tell application \"System Events\"\nset matches to every process whose name contains \"{escaped_name}\"\nif (count of matches) is 0 then error \"T3 Code is not running\"\nset frontmost of item 1 of matches to true\ndelay 0.08\nkeystroke \"{escaped_key}\" using {{{modifiers}}}\nend tell"
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .context("failed to launch osascript")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not allowed to send keystrokes") || stderr.contains("(1002)") {
            bail!(
                "macOS blocked keyboard control. In System Settings → Privacy & Security → \
                 Accessibility, enable the terminal application that launched t3-uwu, then \
                 quit and reopen that terminal"
            );
        }
        bail!("T3 shortcut failed: {}", stderr.trim());
    }
    Ok(())
}
