//! The LLM boundary for the agent.
//!
//! An LLM never touches the hot path and never signs anything. Its entire
//! influence is to return one [`AgentAction`] from a small, closed set. The
//! [`crate::Orchestrator`] then validates and clamps that action against hard
//! ceilings before applying it. This is the core safety idea: even a fully
//! hallucinated model output cannot widen a limit, raise a fee, or move funds,
//! because the action space is constrained and every value is clamped.
//!
//! A production client (OpenAI/Anthropic/local) implements [`LlmClient`] by
//! prompting the model with [`AgentContext`] and parsing a tool call into an
//! [`AgentAction`]. [`ScriptedLlmClient`] is a deterministic implementation for
//! tests and dry runs.

use crate::tuner::{EngineStats, ParamProposal};
use std::cell::Cell;

/// The closed set of actions an agent may propose. Adding a variant is a
/// deliberate, reviewable expansion of the agent's authority.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentAction {
    /// Do nothing this step.
    Hold,
    /// Request a tighter position cap (will be clamped to the ceiling).
    TightenCap { to: u64 },
    /// Request new execution parameters (clamped to ceilings).
    AdjustParams(ParamProposal),
    /// Trip the kill-switch and stop trading.
    TripKillSwitch { reason: String },
    /// Record an observation with no state change.
    Note(String),
}

/// Read-only context handed to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentContext {
    pub stats: EngineStats,
    pub current_params: ParamProposal,
    pub current_cap: u64,
    pub max_cap: u64,
    pub killed: bool,
}

/// Anything that can choose an action from context.
pub trait LlmClient {
    fn decide(&self, ctx: &AgentContext) -> AgentAction;
}

/// Deterministic client that replays a fixed script of actions, then `Hold`s.
/// Used for tests and offline dry-runs of the orchestration logic.
#[derive(Debug)]
pub struct ScriptedLlmClient {
    actions: Vec<AgentAction>,
    cursor: Cell<usize>,
}

impl ScriptedLlmClient {
    pub fn new(actions: Vec<AgentAction>) -> Self {
        Self {
            actions,
            cursor: Cell::new(0),
        }
    }
}

impl LlmClient for ScriptedLlmClient {
    fn decide(&self, _ctx: &AgentContext) -> AgentAction {
        let i = self.cursor.get();
        let action = self.actions.get(i).cloned().unwrap_or(AgentAction::Hold);
        self.cursor.set(i + 1);
        action
    }
}
