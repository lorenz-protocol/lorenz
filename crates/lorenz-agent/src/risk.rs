//! Risk management and the kill-switch.
//!
//! These are hard guarantees enforced in plain code. The position cap and the
//! consecutive-loss kill-switch cannot be widened by the agent layer beyond the
//! configured ceiling; the agent may only tighten them.

use lorenz_core::config::RiskConfig;

/// Outcome of a pre-trade risk check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskDecision {
    pub allow: bool,
    pub reason: String,
}

impl RiskDecision {
    fn allow() -> Self {
        Self {
            allow: true,
            reason: "within limits".to_string(),
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            reason: reason.into(),
        }
    }
}

/// The behaviour the data plane relies on before/after each attempt.
pub trait RiskManager {
    /// Check whether a trade of `notional` base units may proceed.
    fn pre_trade_check(&self, notional: u64) -> RiskDecision;

    /// Update internal state from the realized net profit of a settled attempt.
    fn on_trade_result(&mut self, net_profit: i128);

    /// Whether the kill-switch has tripped.
    fn is_killed(&self) -> bool;
}

/// A deterministic, rule-based risk manager.
#[derive(Debug, Clone)]
pub struct RuleBasedRiskManager {
    cfg: RiskConfig,
    consecutive_losses: u32,
    killed: bool,
    /// Optional tighter position cap set by the agent (never above the config).
    dynamic_cap: Option<u64>,
}

impl RuleBasedRiskManager {
    pub fn new(cfg: RiskConfig) -> Self {
        Self {
            cfg,
            consecutive_losses: 0,
            killed: false,
            dynamic_cap: None,
        }
    }

    /// Effective cap = the tighter of the configured ceiling and any dynamic
    /// cap the agent requested.
    pub fn effective_cap(&self) -> u64 {
        match self.dynamic_cap {
            Some(d) => d.min(self.cfg.max_position),
            None => self.cfg.max_position,
        }
    }

    /// Agent-facing knob: request a tighter cap. Requests above the configured
    /// ceiling are clamped, never honored as written. Returns the value
    /// actually applied.
    pub fn tighten_cap(&mut self, requested: u64) -> u64 {
        let applied = requested.min(self.cfg.max_position);
        self.dynamic_cap = Some(applied);
        applied
    }

    /// Manually trip the kill-switch (e.g. an agent or operator decision).
    pub fn trip_kill_switch(&mut self) {
        self.killed = true;
    }

    /// Manual reset of the kill-switch (operator action, to be logged).
    pub fn reset_kill_switch(&mut self) {
        self.killed = false;
        self.consecutive_losses = 0;
    }

    pub fn consecutive_losses(&self) -> u32 {
        self.consecutive_losses
    }
}

impl RiskManager for RuleBasedRiskManager {
    fn pre_trade_check(&self, notional: u64) -> RiskDecision {
        if self.killed {
            return RiskDecision::deny("kill-switch active");
        }
        if notional > self.effective_cap() {
            return RiskDecision::deny(format!(
                "notional {} exceeds cap {}",
                notional,
                self.effective_cap()
            ));
        }
        RiskDecision::allow()
    }

    fn on_trade_result(&mut self, net_profit: i128) {
        if net_profit < 0 {
            self.consecutive_losses += 1;
            if self.consecutive_losses >= self.cfg.max_consecutive_losses {
                self.killed = true;
            }
        } else {
            self.consecutive_losses = 0;
        }
    }

    fn is_killed(&self) -> bool {
        self.killed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RiskConfig {
        RiskConfig {
            max_position: 1_000_000,
            min_profit: 1,
            max_consecutive_losses: 3,
        }
    }

    #[test]
    fn rejects_oversized_position() {
        let rm = RuleBasedRiskManager::new(cfg());
        assert!(!rm.pre_trade_check(2_000_000).allow);
        assert!(rm.pre_trade_check(1_000_000).allow);
    }

    #[test]
    fn kill_switch_trips_after_consecutive_losses() {
        let mut rm = RuleBasedRiskManager::new(cfg());
        rm.on_trade_result(-10);
        rm.on_trade_result(-10);
        assert!(!rm.is_killed());
        rm.on_trade_result(-10); // third loss -> trip
        assert!(rm.is_killed());
        assert!(!rm.pre_trade_check(1).allow);
    }

    #[test]
    fn a_win_resets_the_loss_streak() {
        let mut rm = RuleBasedRiskManager::new(cfg());
        rm.on_trade_result(-10);
        rm.on_trade_result(-10);
        rm.on_trade_result(100); // win resets
        assert_eq!(rm.consecutive_losses(), 0);
        rm.on_trade_result(-10);
        assert!(!rm.is_killed());
    }

    #[test]
    fn agent_cannot_widen_cap_beyond_ceiling() {
        let mut rm = RuleBasedRiskManager::new(cfg());
        let applied = rm.tighten_cap(5_000_000); // tries to widen
        assert_eq!(applied, 1_000_000); // clamped to ceiling
        let applied = rm.tighten_cap(100_000); // tightens
        assert_eq!(applied, 100_000);
        assert!(!rm.pre_trade_check(200_000).allow);
    }
}
