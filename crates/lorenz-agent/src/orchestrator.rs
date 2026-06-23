//! The orchestrator: the safety layer between an LLM's suggestion and the
//! engine's state.
//!
//! Flow per step:
//! 1. Build a read-only [`AgentContext`] from current stats and limits.
//! 2. Ask the [`LlmClient`] for one [`AgentAction`].
//! 3. **Validate and clamp** it against hard ceilings.
//! 4. Apply the clamped action and record it (with rationale) to the
//!    [`DecisionLedger`].
//!
//! The clamping in step 3 is the whole point: the orchestrator returns the
//! action it *actually applied*, which can differ from what the model asked
//! for. A model can never exceed a ceiling.

use crate::ledger::{Decision, DecisionKind, DecisionLedger};
use crate::llm::{AgentAction, AgentContext, LlmClient};
use crate::risk::{RiskManager, RuleBasedRiskManager};
use crate::tuner::{EngineStats, ParamProposal};

/// Drives an [`LlmClient`] within hard safety bounds.
pub struct Orchestrator<C: LlmClient> {
    llm: C,
    risk: RuleBasedRiskManager,
    params: ParamProposal,
    param_ceiling: ParamProposal,
    ledger: DecisionLedger,
    clock: u64,
}

impl<C: LlmClient> Orchestrator<C> {
    pub fn new(
        llm: C,
        risk: RuleBasedRiskManager,
        initial_params: ParamProposal,
        param_ceiling: ParamProposal,
    ) -> Self {
        Self {
            llm,
            risk,
            params: initial_params,
            param_ceiling,
            ledger: DecisionLedger::new(),
            clock: 0,
        }
    }

    pub fn params(&self) -> ParamProposal {
        self.params
    }

    pub fn current_cap(&self) -> u64 {
        self.risk.effective_cap()
    }

    pub fn is_killed(&self) -> bool {
        self.risk.is_killed()
    }

    pub fn ledger(&self) -> &DecisionLedger {
        &self.ledger
    }

    fn now(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// One orchestration step. Returns the action actually applied (post-clamp).
    pub fn step(&mut self, stats: EngineStats) -> AgentAction {
        let ctx = AgentContext {
            stats,
            current_params: self.params,
            current_cap: self.risk.effective_cap(),
            max_cap: self.risk.effective_cap(),
            killed: self.risk.is_killed(),
        };

        let proposed = self.llm.decide(&ctx);
        self.apply(proposed)
    }

    /// Validate, clamp and apply. Every branch records to the ledger.
    fn apply(&mut self, action: AgentAction) -> AgentAction {
        match action {
            AgentAction::Hold => AgentAction::Hold,

            AgentAction::TightenCap { to } => {
                let before = self.risk.effective_cap();
                // `tighten_cap` clamps to the configured ceiling; a model can
                // never widen beyond it.
                let applied = self.risk.tighten_cap(to);
                let ts = self.now();
                self.ledger.record(Decision {
                    ts,
                    kind: DecisionKind::TightenCap,
                    rationale: format!("agent requested cap {to}; applied {applied} after clamp"),
                    before: Some(before.to_string()),
                    after: Some(applied.to_string()),
                });
                AgentAction::TightenCap { to: applied }
            }

            AgentAction::AdjustParams(req) => {
                let clamped = ParamProposal {
                    priority_fee_lamports: req
                        .priority_fee_lamports
                        .min(self.param_ceiling.priority_fee_lamports),
                    jito_tip_lamports: req
                        .jito_tip_lamports
                        .min(self.param_ceiling.jito_tip_lamports),
                };
                let before = format!("{:?}", self.params);
                self.params = clamped;
                let ts = self.now();
                self.ledger.record(Decision {
                    ts,
                    kind: DecisionKind::ParamChange,
                    rationale: format!("agent requested {req:?}; applied {clamped:?} after clamp"),
                    before: Some(before),
                    after: Some(format!("{clamped:?}")),
                });
                AgentAction::AdjustParams(clamped)
            }

            AgentAction::TripKillSwitch { reason } => {
                self.risk.trip_kill_switch();
                let ts = self.now();
                self.ledger.record(Decision {
                    ts,
                    kind: DecisionKind::KillSwitchTripped,
                    rationale: format!("agent tripped kill-switch: {reason}"),
                    before: None,
                    after: None,
                });
                AgentAction::TripKillSwitch { reason }
            }

            AgentAction::Note(text) => {
                let ts = self.now();
                self.ledger.record(Decision {
                    ts,
                    kind: DecisionKind::Note,
                    rationale: text.clone(),
                    before: None,
                    after: None,
                });
                AgentAction::Note(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ScriptedLlmClient;
    use crate::risk::RuleBasedRiskManager;
    use lorenz_core::config::RiskConfig;

    fn setup(actions: Vec<AgentAction>) -> Orchestrator<ScriptedLlmClient> {
        let risk = RuleBasedRiskManager::new(RiskConfig {
            max_position: 1_000_000,
            min_profit: 1,
            max_consecutive_losses: 3,
        });
        let params = ParamProposal {
            priority_fee_lamports: 100_000,
            jito_tip_lamports: 10_000,
        };
        let ceiling = ParamProposal {
            priority_fee_lamports: 500_000,
            jito_tip_lamports: 50_000,
        };
        Orchestrator::new(ScriptedLlmClient::new(actions), risk, params, ceiling)
    }

    fn stats() -> EngineStats {
        EngineStats {
            land_rate: 0.5,
            avg_net_profit: 10.0,
            samples: 10,
        }
    }

    #[test]
    fn agent_cap_request_is_clamped_to_ceiling() {
        // The model "hallucinates" a cap far above the configured ceiling.
        let mut orch = setup(vec![AgentAction::TightenCap { to: 999_999_999 }]);
        let applied = orch.step(stats());
        assert_eq!(applied, AgentAction::TightenCap { to: 1_000_000 });
        assert_eq!(orch.current_cap(), 1_000_000);
    }

    #[test]
    fn agent_params_are_clamped_to_ceiling() {
        let mut orch = setup(vec![AgentAction::AdjustParams(ParamProposal {
            priority_fee_lamports: 10_000_000,
            jito_tip_lamports: 9_000_000,
        })]);
        let applied = orch.step(stats());
        assert_eq!(
            applied,
            AgentAction::AdjustParams(ParamProposal {
                priority_fee_lamports: 500_000,
                jito_tip_lamports: 50_000,
            })
        );
    }

    #[test]
    fn agent_can_trip_kill_switch_and_it_is_logged() {
        let mut orch = setup(vec![AgentAction::TripKillSwitch {
            reason: "anomaly".into(),
        }]);
        orch.step(stats());
        assert!(orch.is_killed());
        assert_eq!(orch.ledger().len(), 1);
        assert_eq!(
            orch.ledger().entries()[0].kind,
            DecisionKind::KillSwitchTripped
        );
    }

    #[test]
    fn every_action_is_recorded_in_order() {
        let mut orch = setup(vec![
            AgentAction::Note("hello".into()),
            AgentAction::TightenCap { to: 500_000 },
        ]);
        orch.step(stats());
        orch.step(stats());
        assert_eq!(orch.ledger().len(), 2);
        assert_eq!(orch.ledger().entries()[0].ts, 1);
        assert_eq!(orch.ledger().entries()[1].ts, 2);
    }
}
