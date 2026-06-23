//! Append-only decision ledger.
//!
//! Every action the control plane takes (tightening a cap, tripping or
//! resetting the kill-switch, changing a fee parameter) is recorded with a
//! human-readable rationale. This makes the agent layer auditable: you can
//! always answer "why did the system do that?" from the ledger alone.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    TightenCap,
    KillSwitchTripped,
    KillSwitchReset,
    ParamChange,
    Note,
}

/// A single, immutable ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub ts: u64,
    pub kind: DecisionKind,
    /// Why this decision was made, in plain language.
    pub rationale: String,
    /// Optional structured before/after for the changed value.
    pub before: Option<String>,
    pub after: Option<String>,
}

/// Append-only store. There is deliberately no method to mutate or delete past
/// entries.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DecisionLedger {
    entries: Vec<Decision>,
}

impl DecisionLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, decision: Decision) {
        tracing::info!(
            kind = ?decision.kind,
            rationale = %decision.rationale,
            "control-plane decision"
        );
        self.entries.push(decision);
    }

    pub fn entries(&self) -> &[Decision] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_is_append_only_and_ordered() {
        let mut l = DecisionLedger::new();
        l.record(Decision {
            ts: 1,
            kind: DecisionKind::TightenCap,
            rationale: "volatility spike".into(),
            before: Some("1000000".into()),
            after: Some("500000".into()),
        });
        l.record(Decision {
            ts: 2,
            kind: DecisionKind::KillSwitchTripped,
            rationale: "3 consecutive losses".into(),
            before: None,
            after: None,
        });
        assert_eq!(l.len(), 2);
        assert_eq!(l.entries()[0].kind, DecisionKind::TightenCap);
        assert_eq!(l.entries()[1].ts, 2);
    }
}
