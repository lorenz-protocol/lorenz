//! Parameter tuning.
//!
//! The tuner proposes data-plane parameters (priority fee, Jito tip) from
//! recent performance. [`HeuristicTuner`] is a real, deterministic baseline. An
//! LLM-backed tuner is ROADMAP and would implement the same [`ParamTuner`]
//! trait, so it is a drop-in replacement that still flows through the same
//! ledger and the same hard risk ceilings.

/// Rolling stats the tuner reasons about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineStats {
    /// Fraction of submitted bundles that landed, in `[0, 1]`.
    pub land_rate: f64,
    /// Average net profit per submitted trade (base units).
    pub avg_net_profit: f64,
    /// Submitted trades in the window.
    pub samples: u32,
}

/// A proposed parameter change. Always bounded by the caller's ceilings before
/// being applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamProposal {
    pub priority_fee_lamports: u64,
    pub jito_tip_lamports: u64,
}

pub trait ParamTuner {
    fn propose(&self, current: ParamProposal, stats: &EngineStats) -> ParamProposal;
}

/// Deterministic baseline: if too few bundles are landing, bid up fees/tips;
/// if landing reliably, ease off to preserve margin. Bounded multiplicatively
/// so a single window can't blow up the bid.
#[derive(Debug, Clone, Copy)]
pub struct HeuristicTuner {
    pub low_land_rate: f64,
    pub high_land_rate: f64,
    pub max_priority_fee: u64,
    pub max_tip: u64,
}

impl Default for HeuristicTuner {
    fn default() -> Self {
        Self {
            low_land_rate: 0.5,
            high_land_rate: 0.9,
            max_priority_fee: 1_000_000,
            max_tip: 200_000,
        }
    }
}

impl ParamTuner for HeuristicTuner {
    fn propose(&self, current: ParamProposal, stats: &EngineStats) -> ParamProposal {
        // Not enough data: leave parameters untouched.
        if stats.samples < 5 {
            return current;
        }

        let (fee, tip) = if stats.land_rate < self.low_land_rate {
            // Under-landing: bid up by 50%.
            (
                current.priority_fee_lamports.saturating_mul(3) / 2,
                current.jito_tip_lamports.saturating_mul(3) / 2,
            )
        } else if stats.land_rate > self.high_land_rate {
            // Landing reliably: ease off by 10% to keep margin.
            (
                current.priority_fee_lamports.saturating_mul(9) / 10,
                current.jito_tip_lamports.saturating_mul(9) / 10,
            )
        } else {
            (current.priority_fee_lamports, current.jito_tip_lamports)
        };

        ParamProposal {
            priority_fee_lamports: fee.min(self.max_priority_fee),
            jito_tip_lamports: tip.min(self.max_tip),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> ParamProposal {
        ParamProposal {
            priority_fee_lamports: 100_000,
            jito_tip_lamports: 10_000,
        }
    }

    #[test]
    fn holds_with_insufficient_samples() {
        let t = HeuristicTuner::default();
        let stats = EngineStats {
            land_rate: 0.1,
            avg_net_profit: 0.0,
            samples: 2,
        };
        assert_eq!(t.propose(current(), &stats), current());
    }

    #[test]
    fn bids_up_when_under_landing() {
        let t = HeuristicTuner::default();
        let stats = EngineStats {
            land_rate: 0.2,
            avg_net_profit: 50.0,
            samples: 20,
        };
        let p = t.propose(current(), &stats);
        assert!(p.priority_fee_lamports > current().priority_fee_lamports);
        assert!(p.jito_tip_lamports > current().jito_tip_lamports);
    }

    #[test]
    fn eases_off_when_landing_reliably() {
        let t = HeuristicTuner::default();
        let stats = EngineStats {
            land_rate: 0.95,
            avg_net_profit: 50.0,
            samples: 20,
        };
        let p = t.propose(current(), &stats);
        assert!(p.priority_fee_lamports < current().priority_fee_lamports);
    }

    #[test]
    fn never_exceeds_ceiling() {
        let t = HeuristicTuner {
            max_priority_fee: 120_000,
            ..Default::default()
        };
        let stats = EngineStats {
            land_rate: 0.0,
            avg_net_profit: 0.0,
            samples: 100,
        };
        let p = t.propose(current(), &stats);
        assert!(p.priority_fee_lamports <= 120_000);
    }
}
