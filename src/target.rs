use std::fmt;

use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TargetId {
    T3,
    Codex,
}

impl TargetId {
    pub const ALL: [Self; 2] = [Self::T3, Self::Codex];

    pub const fn label(self) -> &'static str {
        match self {
            Self::T3 => "T3 Code",
            Self::Codex => "Codex",
        }
    }
}

impl fmt::Display for TargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::T3 => "t3",
            Self::Codex => "codex",
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentPhase {
    Idle,
    Starting,
    Running,
    WaitingApproval,
    WaitingInput,
    Completed,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ThreadSlot {
    pub id: Option<String>,
    pub title: String,
    pub phase: AgentPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateSource {
    T3Api,
    T3Sqlite,
    CodexAppServer,
}

impl StateSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::T3Api => "T3 API",
            Self::T3Sqlite => "T3 SQLite",
            Self::CodexAppServer => "Codex app-server",
        }
    }
}

pub struct StateSnapshot {
    pub slots: Vec<ThreadSlot>,
    pub source: StateSource,
    pub degraded_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetCommand {
    Next,
    Previous,
    Select(TargetId),
}

impl TargetCommand {
    pub fn parse(action: &str) -> Option<Self> {
        match action {
            "target.next" | "target.cycle" => Some(Self::Next),
            "target.previous" => Some(Self::Previous),
            "target.select.t3" => Some(Self::Select(TargetId::T3)),
            "target.select.codex" => Some(Self::Select(TargetId::Codex)),
            _ => None,
        }
    }

    pub fn resolve(self, current: TargetId, order: &[TargetId]) -> Result<TargetId> {
        if order.is_empty() {
            bail!("target order is empty");
        }
        if let Self::Select(target) = self {
            if !order.contains(&target) {
                bail!("target {target} is not enabled");
            }
            return Ok(target);
        }
        let current_index = order
            .iter()
            .position(|target| *target == current)
            .unwrap_or(0);
        let next_index = match self {
            Self::Next => (current_index + 1) % order.len(),
            Self::Previous => (current_index + order.len() - 1) % order.len(),
            Self::Select(_) => unreachable!(),
        };
        Ok(order[next_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_commands_cycle_and_select() {
        let order = [TargetId::T3, TargetId::Codex];
        assert_eq!(
            TargetCommand::Next.resolve(TargetId::T3, &order).unwrap(),
            TargetId::Codex
        );
        assert_eq!(
            TargetCommand::Previous
                .resolve(TargetId::T3, &order)
                .unwrap(),
            TargetId::Codex
        );
        assert_eq!(
            TargetCommand::parse("target.select.t3")
                .unwrap()
                .resolve(TargetId::Codex, &order)
                .unwrap(),
            TargetId::T3
        );
    }

    #[test]
    fn selecting_a_disabled_target_fails() {
        let error = TargetCommand::Select(TargetId::Codex)
            .resolve(TargetId::T3, &[TargetId::T3])
            .unwrap_err();
        assert!(error.to_string().contains("not enabled"));
    }
}
