use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::target::{TargetId, ThreadSlot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Shortcut {
    key: &'static str,
    command: bool,
    shift: bool,
    option: bool,
    control: bool,
}

impl Shortcut {
    const fn cmd(key: &'static str) -> Self {
        Self::new(key, true, false, false, false)
    }

    const fn cmd_shift(key: &'static str) -> Self {
        Self::new(key, true, true, false, false)
    }

    const fn cmd_option(key: &'static str) -> Self {
        Self::new(key, true, false, true, false)
    }

    const fn control(key: &'static str) -> Self {
        Self::new(key, false, false, false, true)
    }

    const fn control_shift(key: &'static str) -> Self {
        Self::new(key, false, true, false, true)
    }

    const fn new(
        key: &'static str,
        command: bool,
        shift: bool,
        option: bool,
        control: bool,
    ) -> Self {
        Self {
            key,
            command,
            shift,
            option,
            control,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Invocation {
    Activate,
    Shortcut(Shortcut),
    OpenUrl(String),
    None,
}

pub fn run(
    target: TargetId,
    action: &str,
    app_name_contains: &str,
    slots: &[ThreadSlot],
) -> Result<()> {
    match resolve(target, action, slots)? {
        Invocation::Activate => send_shortcut(app_name_contains, None),
        Invocation::Shortcut(shortcut) => send_shortcut(app_name_contains, Some(shortcut)),
        Invocation::OpenUrl(url) => open_url(&url),
        Invocation::None => Ok(()),
    }
}

fn resolve(target: TargetId, action: &str, slots: &[ThreadSlot]) -> Result<Invocation> {
    if action == "none" {
        return Ok(Invocation::None);
    }
    if action == "app.activate" {
        return Ok(Invocation::Activate);
    }
    if let Some(index) = action.strip_prefix("thread.jump.") {
        let index = index
            .parse::<usize>()
            .ok()
            .filter(|index| (1..=6).contains(index))
            .ok_or_else(|| anyhow::anyhow!("invalid thread jump action {action:?}"))?;
        return match target {
            TargetId::T3 => Ok(Invocation::Shortcut(Shortcut::cmd(match index {
                1 => "1",
                2 => "2",
                3 => "3",
                4 => "4",
                5 => "5",
                6 => "6",
                _ => unreachable!(),
            }))),
            TargetId::Codex => {
                let thread_id = slots
                    .get(index - 1)
                    .and_then(|slot| slot.id.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("Codex thread slot {index} is empty"))?;
                Ok(Invocation::OpenUrl(format!(
                    "codex://threads/{}",
                    percent_encode(thread_id)
                )))
            }
        };
    }

    let shortcut = match (target, action) {
        (_, "thread.previous") => Shortcut::cmd_shift("["),
        (_, "thread.next") => Shortcut::cmd_shift("]"),

        (TargetId::T3, "chat.new") => Shortcut::cmd("n"),
        (TargetId::T3, "chat.newLocal") => Shortcut::cmd_shift("n"),
        (TargetId::T3, "commandPalette.toggle") => Shortcut::cmd("k"),
        (TargetId::T3, "diff.toggle") => Shortcut::cmd("d"),
        (TargetId::T3, "terminal.toggle") => Shortcut::cmd("j"),
        (TargetId::T3, "preview.toggle") => Shortcut::cmd_shift("j"),
        (TargetId::T3, "preview.refresh") => Shortcut::cmd("r"),
        (TargetId::T3, "modelPicker.toggle") => Shortcut::cmd_shift("m"),
        (TargetId::T3, "sidebar.toggle") => Shortcut::cmd("b"),
        (TargetId::T3, "rightPanel.toggle") => Shortcut::cmd_option("b"),

        (TargetId::Codex, "chat.new" | "chat.newLocal") => {
            return Ok(Invocation::OpenUrl("codex://threads/new".into()));
        }
        (TargetId::Codex, "commandPalette.toggle") => Shortcut::cmd("k"),
        (TargetId::Codex, "diff.toggle") => Shortcut::control_shift("g"),
        (TargetId::Codex, "terminal.toggle") => Shortcut::control("`"),
        (TargetId::Codex, "preview.toggle") => Shortcut::cmd("j"),
        (TargetId::Codex, "preview.refresh") => Shortcut::cmd("r"),
        (TargetId::Codex, "modelPicker.toggle") => Shortcut::cmd("k"),
        (TargetId::Codex, "sidebar.toggle") => Shortcut::cmd("b"),
        (TargetId::Codex, "rightPanel.toggle") => Shortcut::cmd_option("b"),
        (_, other) => bail!("unknown action {other:?} for target {target}"),
    };
    Ok(Invocation::Shortcut(shortcut))
}

fn open_url(url: &str) -> Result<()> {
    let status = Command::new("open")
        .arg(url)
        .status()
        .context("failed to launch the macOS open command")?;
    if !status.success() {
        bail!("failed to open {url}");
    }
    Ok(())
}

fn send_shortcut(app_name_contains: &str, shortcut: Option<Shortcut>) -> Result<()> {
    let escaped_name = app_name_contains.replace('\\', "\\\\").replace('"', "\\\"");
    let keystroke = shortcut.map_or_else(String::new, |shortcut| {
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
        let escaped_key = shortcut.key.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\ndelay 0.08\nkeystroke \"{escaped_key}\" using {{{modifiers}}}")
    });
    let script = format!(
        "tell application \"System Events\"\nset matches to every process whose name contains \"{escaped_name}\"\nif (count of matches) is 0 then error \"{escaped_name} is not running\"\nset frontmost of item 1 of matches to true{keystroke}\nend tell"
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
                 Accessibility, enable the terminal application that launched uwu-vibe, then \
                 quit and reopen that terminal"
            );
        }
        bail!("{app_name_contains} shortcut failed: {}", stderr.trim());
    }
    Ok(())
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::AgentPhase;

    fn slot(id: &str) -> ThreadSlot {
        ThreadSlot {
            id: Some(id.into()),
            title: "Thread".into(),
            phase: AgentPhase::Idle,
        }
    }

    #[test]
    fn t3_thread_jumps_remain_keyboard_shortcuts() {
        assert_eq!(
            resolve(TargetId::T3, "thread.jump.2", &[]).unwrap(),
            Invocation::Shortcut(Shortcut::cmd("2"))
        );
    }

    #[test]
    fn codex_thread_jumps_use_deep_links() {
        assert_eq!(
            resolve(TargetId::Codex, "thread.jump.1", &[slot("abc/123")]).unwrap(),
            Invocation::OpenUrl("codex://threads/abc%2F123".into())
        );
    }

    #[test]
    fn an_empty_codex_slot_is_reported() {
        let error = resolve(TargetId::Codex, "thread.jump.1", &[]).unwrap_err();
        assert!(error.to_string().contains("slot 1 is empty"));
    }
}
