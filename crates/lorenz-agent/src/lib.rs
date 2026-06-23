//! Control plane primitives.
//!
//! The control plane is the "brain that governs", never the "hand that
//! executes". Nothing in here runs on the hot path: these types decide *limits*
//! and *parameters* that the deterministic data plane then enforces. An LLM
//! agent (roadmap) would sit on top of these primitives, but the primitives
//! themselves are plain, testable Rust so the safety properties do not depend
//! on a model's output.
//!
//! Two ideas hold the design together:
//! - [`RiskManager`] owns hard guarantees (kill-switch, position caps). Even a
//!   misbehaving agent cannot widen these beyond the configured ceiling.
//! - [`DecisionLedger`] records *why* every change happened, so the agent layer
//!   is itself auditable after the fact.

pub mod ledger;
pub mod llm;
pub mod orchestrator;
pub mod risk;
pub mod tuner;

pub use ledger::{Decision, DecisionKind, DecisionLedger};
pub use llm::{AgentAction, AgentContext, LlmClient, ScriptedLlmClient};
pub use orchestrator::Orchestrator;
pub use risk::{RiskDecision, RiskManager, RuleBasedRiskManager};
pub use tuner::{EngineStats, HeuristicTuner, ParamProposal, ParamTuner};
