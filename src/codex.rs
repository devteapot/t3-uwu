use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    config::Config,
    target::{AgentPhase, StateSnapshot, StateSource, ThreadSlot},
};

const HOOK_ACTIVE_TTL_MS: u64 = 15 * 60 * 1000;

pub struct CodexState {
    server: CodexAppServer,
    source_kinds: Vec<String>,
    phases: HashMap<String, CachedPhase>,
}

impl CodexState {
    pub fn open(config: &Config) -> Result<Self> {
        Ok(Self {
            server: CodexAppServer::start(&config.codex_bin)?,
            source_kinds: config.codex_source_kinds.clone(),
            phases: HashMap::new(),
        })
    }

    pub fn slots(&mut self) -> Result<StateSnapshot> {
        let listed: ThreadListResponse = self.server.request(
            "thread/list",
            json!({
                "limit": 6,
                "sortKey": "recency_at",
                "sortDirection": "desc",
                "sourceKinds": &self.source_kinds,
                "archived": false,
                "useStateDbOnly": true
            }),
        )?;
        let hooks = read_hook_events();
        let mut slots = Vec::with_capacity(listed.data.len());
        for thread in listed.data {
            let runtime_phase = phase_from_status(&thread.status);
            let cache_key = thread.id.clone();
            let needs_read = runtime_phase.is_none()
                && self
                    .phases
                    .get(&cache_key)
                    .is_none_or(|cached| cached.thread_updated_at != thread.updated_at);
            if needs_read && let Ok(phase) = self.read_latest_phase(&thread.id) {
                self.phases.insert(
                    cache_key.clone(),
                    CachedPhase {
                        thread_updated_at: thread.updated_at,
                        phase,
                    },
                );
            }
            let mut phase = runtime_phase
                .or_else(|| self.phases.get(&cache_key).map(|cached| cached.phase))
                .unwrap_or(AgentPhase::Idle);
            if let Some(hook) = hooks
                .get(&thread.id)
                .or_else(|| hooks.get(&thread.session_id))
                && hook.applies_to(thread.updated_at)
            {
                phase = hook.phase;
            }
            self.phases.insert(
                cache_key,
                CachedPhase {
                    thread_updated_at: thread.updated_at,
                    phase,
                },
            );
            let title = thread
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    let preview = thread.preview.trim();
                    if preview.is_empty() {
                        "Untitled Codex chat".into()
                    } else {
                        preview.chars().take(80).collect()
                    }
                });
            slots.push(ThreadSlot {
                id: Some(thread.id),
                title,
                phase,
            });
        }
        Ok(StateSnapshot {
            slots,
            source: StateSource::CodexAppServer,
            degraded_reason: None,
        })
    }

    fn read_latest_phase(&mut self, thread_id: &str) -> Result<AgentPhase> {
        let response: ThreadReadResponse = self.server.request(
            "thread/read",
            json!({ "threadId": thread_id, "includeTurns": true }),
        )?;
        Ok(response
            .thread
            .turns
            .last()
            .map_or(AgentPhase::Idle, |turn| match turn.status.as_str() {
                "inProgress" => AgentPhase::Running,
                "completed" => AgentPhase::Completed,
                "failed" => AgentPhase::Failed,
                "interrupted" => AgentPhase::Idle,
                _ => AgentPhase::Idle,
            }))
    }
}

struct CodexAppServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl CodexAppServer {
    fn start(binary: &str) -> Result<Self> {
        let mut child = Command::new(binary)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start Codex app-server with {binary:?}"))?;
        let stdin = child
            .stdin
            .take()
            .context("Codex app-server did not expose stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("Codex app-server did not expose stdout")?;
        let mut server = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        let _: Value = server.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "uwu_vibe",
                    "title": "uwu-vibe",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        server.notify("initialized", json!({}))?;
        Ok(server)
    }

    fn request<T: for<'de> Deserialize<'de>>(&mut self, method: &str, params: Value) -> Result<T> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&json!({ "method": method, "id": id, "params": params }))?;
        loop {
            let mut line = String::new();
            let length = self
                .stdout
                .read_line(&mut line)
                .context("failed reading from Codex app-server")?;
            if length == 0 {
                bail!("Codex app-server exited while handling {method}");
            }
            let message: Value =
                serde_json::from_str(&line).context("Codex app-server emitted invalid JSON")?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("Codex app-server rejected {method}: {error}");
            }
            let result = message
                .get("result")
                .cloned()
                .context("Codex app-server response has no result")?;
            return serde_json::from_value(result)
                .with_context(|| format!("invalid Codex app-server response for {method}"));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({ "method": method, "params": params }))
    }

    fn write_message(&mut self, message: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListResponse {
    data: Vec<CodexThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexThread {
    id: String,
    session_id: String,
    name: Option<String>,
    preview: String,
    updated_at: i64,
    status: CodexThreadStatus,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum CodexThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active {
        #[serde(default, rename = "activeFlags")]
        active_flags: Vec<String>,
    },
}

#[derive(Deserialize)]
struct ThreadReadResponse {
    thread: ThreadWithTurns,
}

#[derive(Deserialize)]
struct ThreadWithTurns {
    turns: Vec<CodexTurn>,
}

#[derive(Deserialize)]
struct CodexTurn {
    status: String,
}

#[derive(Clone, Copy)]
struct CachedPhase {
    thread_updated_at: i64,
    phase: AgentPhase,
}

fn phase_from_status(status: &CodexThreadStatus) -> Option<AgentPhase> {
    match status {
        CodexThreadStatus::NotLoaded => None,
        CodexThreadStatus::Idle => Some(AgentPhase::Idle),
        CodexThreadStatus::SystemError => Some(AgentPhase::Failed),
        CodexThreadStatus::Active { active_flags } => {
            if active_flags.iter().any(|flag| flag == "waitingOnApproval") {
                Some(AgentPhase::WaitingApproval)
            } else if active_flags.iter().any(|flag| flag == "waitingOnUserInput") {
                Some(AgentPhase::WaitingInput)
            } else {
                Some(AgentPhase::Running)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HookState {
    session_id: String,
    turn_id: Option<String>,
    phase: AgentPhase,
    updated_at_ms: u64,
}

impl HookState {
    fn applies_to(&self, thread_updated_at: i64) -> bool {
        let thread_updated_at_ms = thread_updated_at.max(0) as u64 * 1000;
        if self.updated_at_ms + 5_000 < thread_updated_at_ms {
            return false;
        }
        if matches!(
            self.phase,
            AgentPhase::Running | AgentPhase::WaitingApproval | AgentPhase::WaitingInput
        ) {
            return now_ms().saturating_sub(self.updated_at_ms) <= HOOK_ACTIVE_TTL_MS;
        }
        true
    }
}

pub fn record_hook_event(input: &str) -> Result<bool> {
    let event: Value = serde_json::from_str(input).context("invalid Codex hook JSON")?;
    let Some(session_id) = event.get("session_id").and_then(Value::as_str) else {
        bail!("Codex hook event has no session_id");
    };
    let event_name = event
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool_name = event.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let Some(phase) = hook_phase(event_name, tool_name) else {
        return Ok(false);
    };
    let state = HookState {
        session_id: session_id.into(),
        turn_id: event
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        phase,
        updated_at_ms: now_ms(),
    };
    let directory = hook_state_dir();
    fs::create_dir_all(&directory).with_context(|| {
        format!(
            "failed to create Codex hook state directory {}",
            directory.display()
        )
    })?;
    let path = directory.join(format!("{}.json", safe_filename(session_id)));
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        safe_filename(session_id),
        std::process::id()
    ));
    fs::write(&temporary, serde_json::to_vec(&state)?)
        .with_context(|| format!("failed to write Codex hook state {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("failed to publish Codex hook state {}", path.display()))?;
    Ok(true)
}

fn hook_phase(event_name: &str, tool_name: &str) -> Option<AgentPhase> {
    match event_name {
        "SessionStart" => Some(AgentPhase::Idle),
        "UserPromptSubmit" => Some(AgentPhase::Running),
        "PermissionRequest" => Some(AgentPhase::WaitingApproval),
        "PreToolUse" if tool_name.contains("request_user_input") => Some(AgentPhase::WaitingInput),
        "PostToolUse" if tool_name.contains("request_user_input") => Some(AgentPhase::Running),
        "Stop" => Some(AgentPhase::Completed),
        _ => None,
    }
}

fn read_hook_events() -> HashMap<String, HookState> {
    let mut states = read_hook_events_from(legacy_hook_state_dir());
    states.extend(read_hook_events_from(hook_state_dir()));
    states
}

fn read_hook_events_from(directory: PathBuf) -> HashMap<String, HookState> {
    fs::read_dir(directory).map_or_else(
        |_| HashMap::new(),
        |entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| fs::read(entry.path()).ok())
                .filter_map(|source| serde_json::from_slice::<HookState>(&source).ok())
                .map(|state| (state.session_id.clone(), state))
                .collect()
        },
    )
}

fn hook_state_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("uwu-vibe")
        .join("codex-events")
}

fn legacy_hook_state_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("t3-uwu")
        .join("codex-events")
}

fn safe_filename(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_flags_resolve_with_blockers_first() {
        assert_eq!(
            phase_from_status(&CodexThreadStatus::Active {
                active_flags: vec!["waitingOnApproval".into(), "waitingOnUserInput".into()]
            }),
            Some(AgentPhase::WaitingApproval)
        );
        assert_eq!(
            phase_from_status(&CodexThreadStatus::Active {
                active_flags: Vec::new()
            }),
            Some(AgentPhase::Running)
        );
    }

    #[test]
    fn hook_events_map_to_live_phases() {
        assert_eq!(
            hook_phase("PermissionRequest", "Bash"),
            Some(AgentPhase::WaitingApproval)
        );
        assert_eq!(
            hook_phase("PreToolUse", "request_user_input"),
            Some(AgentPhase::WaitingInput)
        );
        assert_eq!(hook_phase("Stop", ""), Some(AgentPhase::Completed));
        assert_eq!(hook_phase("PostToolUse", "Bash"), None);
    }

    #[test]
    fn filenames_do_not_escape_the_state_directory() {
        assert_eq!(safe_filename("../../thread id"), ".._.._thread_id");
    }
}
