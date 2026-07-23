use std::{env, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use keyring::{Entry, Error as KeyringError};
use reqwest::{StatusCode, Url, blocking::Client};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::{
    config::{Config, T3StateSource},
    target::{AgentPhase, StateSnapshot, StateSource, ThreadSlot},
};

const KEYRING_SERVICE: &str = "devteapot.uwu-vibe";
const LEGACY_KEYRING_SERVICE: &str = "devteapot.t3-uwu";
const KEYRING_ACCOUNT: &str = "t3-api";
const DEFAULT_TOKEN_ENV: &str = "UWU_VIBE_T3_BEARER_TOKEN";
const LEGACY_TOKEN_ENV: &str = "T3_UWU_BEARER_TOKEN";
const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const BOOTSTRAP_TOKEN_TYPE: &str = "urn:t3:params:oauth:token-type:environment-bootstrap";
const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";

pub struct T3State {
    api: Option<ApiState>,
    sqlite: Option<SqliteState>,
}

impl T3State {
    pub fn open(config: &Config) -> Result<Self> {
        match config.t3_state_source {
            T3StateSource::Api => Ok(Self {
                api: Some(ApiState::from_config(config)?.context(
                    "T3 API is not paired; run `uwu-vibe pair` or set the configured bearer-token environment variable",
                )?),
                sqlite: None,
            }),
            T3StateSource::Sqlite => Ok(Self {
                api: None,
                sqlite: Some(SqliteState::open(&config.t3_database)?),
            }),
            T3StateSource::Auto => {
                let api = match ApiState::from_config(config) {
                    Ok(api) => api,
                    Err(error) => {
                        eprintln!("T3 API setup unavailable; using SQLite: {error:#}");
                        None
                    }
                };
                let sqlite = match SqliteState::open(&config.t3_database) {
                    Ok(state) => Some(state),
                    Err(error) if api.is_some() => {
                        eprintln!("SQLite fallback unavailable: {error:#}");
                        None
                    }
                    Err(error) => return Err(error),
                };
                Ok(Self { api, sqlite })
            }
        }
    }

    pub fn slots(&self) -> Result<StateSnapshot> {
        if let Some(api) = &self.api {
            match api.slots() {
                Ok(slots) => {
                    return Ok(StateSnapshot {
                        slots,
                        source: StateSource::T3Api,
                        degraded_reason: None,
                    });
                }
                Err(api_error) => {
                    if let Some(sqlite) = &self.sqlite {
                        return Ok(StateSnapshot {
                            slots: sqlite.slots()?,
                            source: StateSource::T3Sqlite,
                            degraded_reason: Some(format!("{api_error:#}")),
                        });
                    }
                    return Err(api_error);
                }
            }
        }

        let sqlite = self
            .sqlite
            .as_ref()
            .context("no T3 state backend is available")?;
        Ok(StateSnapshot {
            slots: sqlite.slots()?,
            source: StateSource::T3Sqlite,
            degraded_reason: None,
        })
    }
}

struct SqliteState {
    connection: Connection,
}

impl SqliteState {
    fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("failed to open T3 state database {}", path.display()))?;
        connection.busy_timeout(Duration::from_millis(100))?;
        Ok(Self { connection })
    }

    fn slots(&self) -> Result<Vec<ThreadSlot>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT t.thread_id,
                    t.title,
                    t.pending_approval_count,
                    t.pending_user_input_count,
                    COALESCE(s.status, ''),
                    COALESCE(v.state, ''),
                    v.completed_at
             FROM projection_threads t
             LEFT JOIN projection_thread_sessions s ON s.thread_id = t.thread_id
             LEFT JOIN projection_turns v ON v.row_id = (
               SELECT row_id FROM projection_turns
               WHERE thread_id = t.thread_id ORDER BY requested_at DESC LIMIT 1
             )
             WHERE t.deleted_at IS NULL AND t.archived_at IS NULL
             ORDER BY
               CASE
                 WHEN t.settled_override = 'settled'
                  AND t.pending_approval_count = 0
                  AND t.pending_user_input_count = 0
                  AND COALESCE(s.status, '') NOT IN ('starting', 'running')
                 THEN 1 ELSE 0
               END ASC,
               CASE
                 WHEN t.settled_override IS NULL OR t.settled_override != 'settled'
                   OR t.pending_approval_count > 0 OR t.pending_user_input_count > 0
                   OR COALESCE(s.status, '') IN ('starting', 'running')
                 THEN t.created_at
               END DESC,
               CASE WHEN t.settled_override = 'settled'
                 THEN COALESCE(t.latest_user_message_at, t.updated_at)
               END DESC,
               t.thread_id ASC
             LIMIT 3",
        )?;
        let rows = statement.query_map([], |row| {
            let approval: i64 = row.get(2)?;
            let input: i64 = row.get(3)?;
            let session: String = row.get(4)?;
            let turn: String = row.get(5)?;
            let completed_at: Option<String> = row.get(6)?;
            Ok(ThreadSlot {
                id: Some(row.get(0)?),
                title: row.get(1)?,
                phase: resolve_phase(
                    approval > 0,
                    input > 0,
                    &session,
                    &turn,
                    completed_at.is_some(),
                ),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed reading T3 thread slots")
    }
}

struct ApiState {
    origin: String,
    token: String,
    client: Client,
}

impl ApiState {
    fn from_config(config: &Config) -> Result<Option<Self>> {
        let Some(origin) = resolve_origin(config)? else {
            return Ok(None);
        };
        let Some(token) = resolve_token(config)? else {
            return Ok(None);
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .context("failed to create T3 API client")?;
        Ok(Some(Self {
            origin,
            token,
            client,
        }))
    }

    fn slots(&self) -> Result<Vec<ThreadSlot>> {
        let endpoint = format!("{}/api/orchestration/shell", self.origin);
        let response = self
            .client
            .get(endpoint)
            .bearer_auth(&self.token)
            .send()
            .context("failed to reach the T3 API")?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            bail!("T3 API authorization expired or was revoked; run `uwu-vibe pair` again");
        }
        let response = response
            .error_for_status()
            .context("T3 API rejected the shell snapshot request")?;
        let snapshot: ApiShellSnapshot = response
            .json()
            .context("T3 API returned an invalid shell snapshot")?;
        Ok(slots_from_api(snapshot.threads))
    }
}

#[derive(Deserialize)]
struct RuntimeState {
    origin: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiShellSnapshot {
    threads: Vec<ApiThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiThread {
    id: String,
    title: String,
    created_at: String,
    updated_at: String,
    latest_user_message_at: Option<String>,
    settled_override: Option<String>,
    has_pending_approvals: bool,
    has_pending_user_input: bool,
    session: Option<ApiSession>,
    latest_turn: Option<ApiTurn>,
}

#[derive(Deserialize)]
struct ApiSession {
    status: String,
}

#[derive(Deserialize)]
struct ApiTurn {
    state: String,
    #[serde(rename = "completedAt")]
    completed_at: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
}

pub fn pair(pairing_url: &str) -> Result<String> {
    let (origin, credential) = parse_pairing_url(pairing_url)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to create T3 pairing client")?;
    let endpoint = format!("{origin}/oauth/token");
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", TOKEN_EXCHANGE_GRANT),
            ("subject_token", credential.as_str()),
            ("subject_token_type", BOOTSTRAP_TOKEN_TYPE),
            ("requested_token_type", ACCESS_TOKEN_TYPE),
            ("scope", "orchestration:read"),
            ("client_label", "uwu-vibe"),
            ("client_device_type", "desktop"),
            ("client_os", "macOS"),
        ])
        .send()
        .context("failed to reach T3 while pairing")?;
    if !response.status().is_success() {
        bail!(
            "T3 rejected the pairing credential (HTTP {}); create a fresh pairing link and try again",
            response.status()
        );
    }
    let token: TokenResponse = response
        .json()
        .context("T3 returned an invalid pairing response")?;
    if !token.token_type.eq_ignore_ascii_case("bearer") {
        bail!("T3 returned unsupported token type {:?}", token.token_type);
    }
    keyring_entry(KEYRING_SERVICE)?
        .set_password(&token.access_token)
        .context("failed to save the T3 credential in the system keychain")?;
    Ok(origin)
}

pub fn unpair() -> Result<bool> {
    let mut removed = false;
    for service in [KEYRING_SERVICE, LEGACY_KEYRING_SERVICE] {
        match keyring_entry(service)?.delete_credential() {
            Ok(()) => removed = true,
            Err(KeyringError::NoEntry) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to remove the T3 credential from Keychain service {service}")
                });
            }
        }
    }
    Ok(removed)
}

fn resolve_origin(config: &Config) -> Result<Option<String>> {
    if let Some(configured) = config
        .t3_http_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return normalize_origin(configured).map(Some);
    }
    let source = match std::fs::read_to_string(&config.t3_runtime) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to read T3 runtime file {}",
                    config.t3_runtime.display()
                )
            });
        }
    };
    let runtime: RuntimeState = serde_json::from_str(&source).with_context(|| {
        format!(
            "failed to parse T3 runtime file {}",
            config.t3_runtime.display()
        )
    })?;
    normalize_origin(&runtime.origin).map(Some)
}

fn resolve_token(config: &Config) -> Result<Option<String>> {
    if !config.t3_bearer_token_env.is_empty() {
        if let Some(token) = token_from_env(&config.t3_bearer_token_env)? {
            return Ok(Some(token));
        }
        if config.t3_bearer_token_env == DEFAULT_TOKEN_ENV
            && let Some(token) = token_from_env(LEGACY_TOKEN_ENV)?
        {
            return Ok(Some(token));
        }
    }
    for service in [KEYRING_SERVICE, LEGACY_KEYRING_SERVICE] {
        match keyring_entry(service)?.get_password() {
            Ok(token) => return Ok(Some(token)),
            Err(KeyringError::NoEntry) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to read the T3 credential from Keychain service {service}")
                });
            }
        }
    }
    Ok(None)
}

fn token_from_env(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(token) if !token.trim().is_empty() => Ok(Some(token)),
        Ok(_) | Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => {
            Err(error).context("configured T3 bearer-token environment variable is invalid")
        }
    }
}

fn keyring_entry(service: &str) -> Result<Entry> {
    Entry::new(service, KEYRING_ACCOUNT).context("failed to open the system keychain")
}

fn parse_pairing_url(input: &str) -> Result<(String, String)> {
    let url = Url::parse(input.trim()).context("invalid T3 pairing URL")?;
    let credential = url
        .fragment()
        .and_then(|fragment| {
            form_pairs(fragment).find_map(|(key, value)| (key == "token").then_some(value))
        })
        .or_else(|| {
            url.query_pairs()
                .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))
        })
        .filter(|value| !value.is_empty())
        .context("pairing URL does not contain a token")?;
    let hosted_origin = url
        .query_pairs()
        .find_map(|(key, value)| (key == "host").then(|| value.into_owned()));
    let origin = match hosted_origin {
        Some(host) => normalize_origin(&host)?,
        None => normalize_origin(url.as_str())?,
    };
    Ok((origin, credential))
}

fn form_pairs(value: &str) -> impl Iterator<Item = (String, String)> + '_ {
    value.split('&').filter_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let parsed = Url::parse(&format!("http://local/?{key}={value}")).ok()?;
        parsed
            .query_pairs()
            .next()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
    })
}

fn normalize_origin(input: &str) -> Result<String> {
    let mut url = Url::parse(input.trim()).context("invalid T3 server URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "T3 server URL must use http or https"
    );
    anyhow::ensure!(url.host_str().is_some(), "T3 server URL has no host");
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn slots_from_api(threads: Vec<ApiThread>) -> Vec<ThreadSlot> {
    let (mut active, mut settled): (Vec<_>, Vec<_>) = threads.into_iter().partition(|thread| {
        thread.settled_override.as_deref() != Some("settled")
            || thread.has_pending_approvals
            || thread.has_pending_user_input
            || matches!(
                thread
                    .session
                    .as_ref()
                    .map(|session| session.status.as_str()),
                Some("starting" | "running")
            )
    });
    active.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    settled.sort_by(|left, right| {
        let left_activity = left
            .latest_user_message_at
            .as_deref()
            .unwrap_or(&left.updated_at);
        let right_activity = right
            .latest_user_message_at
            .as_deref()
            .unwrap_or(&right.updated_at);
        right_activity
            .cmp(left_activity)
            .then_with(|| left.id.cmp(&right.id))
    });
    active
        .into_iter()
        .chain(settled)
        .take(3)
        .map(|thread| {
            let session = thread
                .session
                .as_ref()
                .map_or("", |session| session.status.as_str());
            let turn = thread
                .latest_turn
                .as_ref()
                .map_or("", |turn| turn.state.as_str());
            let completed = thread
                .latest_turn
                .as_ref()
                .is_some_and(|turn| turn.completed_at.is_some());
            ThreadSlot {
                id: Some(thread.id),
                title: thread.title,
                phase: resolve_phase(
                    thread.has_pending_approvals,
                    thread.has_pending_user_input,
                    session,
                    turn,
                    completed,
                ),
            }
        })
        .collect()
}

fn resolve_phase(
    approval: bool,
    input: bool,
    session: &str,
    turn: &str,
    completed: bool,
) -> AgentPhase {
    if approval {
        return AgentPhase::WaitingApproval;
    }
    if input {
        return AgentPhase::WaitingInput;
    }
    if matches!(session, "error" | "failed") || matches!(turn, "error" | "failed") {
        return AgentPhase::Failed;
    }
    if session == "starting" {
        return AgentPhase::Starting;
    }
    if session == "running" || matches!(turn, "running" | "in_progress") {
        return AgentPhase::Running;
    }
    if turn == "completed"
        || (turn == "interrupted" && completed)
        || matches!(session, "ready" | "idle")
    {
        return AgentPhase::Completed;
    }
    AgentPhase::Idle
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn api_thread(id: &str, created_at: &str, settled: bool) -> ApiThread {
        ApiThread {
            id: id.into(),
            title: id.into(),
            created_at: created_at.into(),
            updated_at: created_at.into(),
            latest_user_message_at: None,
            settled_override: settled.then(|| "settled".into()),
            has_pending_approvals: false,
            has_pending_user_input: false,
            session: None,
            latest_turn: None,
        }
    }

    #[test]
    fn blockers_take_priority_over_running() {
        assert_eq!(
            resolve_phase(true, false, "running", "running", false),
            AgentPhase::WaitingApproval
        );
        assert_eq!(
            resolve_phase(false, true, "running", "running", false),
            AgentPhase::WaitingInput
        );
    }

    #[test]
    fn recognizes_terminal_states() {
        assert_eq!(
            resolve_phase(false, false, "error", "", false),
            AgentPhase::Failed
        );
        assert_eq!(
            resolve_phase(false, false, "ready", "completed", true),
            AgentPhase::Completed
        );
    }

    #[test]
    fn parses_direct_and_hosted_pairing_urls() {
        let direct = parse_pairing_url("http://127.0.0.1:3773/pair#token=secret%201").unwrap();
        assert_eq!(direct, ("http://127.0.0.1:3773".into(), "secret 1".into()));

        let hosted = parse_pairing_url(
            "https://app.t3.codes/pair?host=https%3A%2F%2Ft3.example.test%3A44342%2F#token=abc",
        )
        .unwrap();
        assert_eq!(
            hosted,
            ("https://t3.example.test:44342".into(), "abc".into())
        );
    }

    #[test]
    fn api_slots_put_active_threads_before_settled_tail() {
        let mut waiting = api_thread("waiting", "2026-07-22T09:00:00Z", true);
        waiting.has_pending_approvals = true;
        let slots = slots_from_api(vec![
            api_thread("settled", "2026-07-22T11:00:00Z", true),
            api_thread("new", "2026-07-22T10:00:00Z", false),
            waiting,
            api_thread("old", "2026-07-22T08:00:00Z", false),
        ]);
        assert_eq!(
            slots
                .iter()
                .map(|slot| slot.title.as_str())
                .collect::<Vec<_>>(),
            ["new", "waiting", "old"]
        );
        assert_eq!(slots[1].phase, AgentPhase::WaitingApproval);
    }

    #[test]
    fn api_backend_sends_bearer_token_and_decodes_shell() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0_u8; 1024];
                let length = stream.read(&mut chunk).unwrap();
                assert!(length > 0);
                request.extend_from_slice(&chunk[..length]);
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("GET /api/orchestration/shell HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );

            let body = r#"{
                "threads": [{
                    "id": "thread-1",
                    "title": "Needs input",
                    "createdAt": "2026-07-22T10:00:00Z",
                    "updatedAt": "2026-07-22T10:00:00Z",
                    "latestUserMessageAt": null,
                    "settledOverride": null,
                    "hasPendingApprovals": false,
                    "hasPendingUserInput": true,
                    "session": null,
                    "latestTurn": null
                }]
            }"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let api = ApiState {
            origin: format!("http://{address}"),
            token: "test-token".into(),
            client: Client::builder()
                .timeout(Duration::from_secs(1))
                .build()
                .unwrap(),
        };
        let slots = api.slots().unwrap();
        server.join().unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].title, "Needs input");
        assert_eq!(slots[0].phase, AgentPhase::WaitingInput);
    }
}
