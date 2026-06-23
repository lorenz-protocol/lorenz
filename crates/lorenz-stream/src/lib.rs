//! Streaming layer: turn a sequence of pool-state updates into detected
//! arbitrage candidates, independently of where the bytes come from.
//!
//! The transport is abstracted behind [`PoolSnapshotSource`]. Two sources:
//! - [`replay::ReplaySource`] — a real, deterministic source over recorded
//!   snapshots. This is the working consumer used in tests and the backtester,
//!   and it genuinely drives the detector end-to-end.
//! - [`geyser`] — the production transport seam for a live Yellowstone/Geyser
//!   gRPC subscription. It is a typed, documented integration point; wiring the
//!   actual `yellowstone-grpc-client` is a production step (see its docs). We
//!   keep the heavyweight Solana client dependency out of the core on purpose,
//!   so the deterministic crates stay light and fast to build.

pub mod geyser;
pub mod replay;

use lorenz_dex::Pool;
use lorenz_graph::{ArbitrageGraph, Cycle};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("stream transport not implemented in this build: {0}")]
    NotImplemented(&'static str),
    #[error("transport error: {0}")]
    Transport(String),
}

/// A snapshot of all pools the engine is watching at a point in time (slot).
/// Pools may be of any kind (constant-product or concentrated-liquidity).
#[derive(Debug, Clone, PartialEq)]
pub struct PoolSnapshot {
    pub slot: u64,
    pub pools: Vec<Pool>,
}

/// A source of pool snapshots. A Geyser subscription, a replay file, or a test
/// fixture all implement this identically.
pub trait PoolSnapshotSource {
    /// Next snapshot, or `None` when the source is exhausted.
    fn next_snapshot(&mut self) -> Option<PoolSnapshot>;
}

/// Drive the detector over a source. For every snapshot that contains a
/// profitable cycle, `on_cycle` is invoked with the slot and the cycle. This is
/// the exact analysis step the live engine runs, just fed from an abstract
/// source.
pub fn detect_stream<S, F>(mut source: S, probe_size: u128, mut on_cycle: F) -> usize
where
    S: PoolSnapshotSource,
    F: FnMut(u64, Cycle),
{
    let mut detected = 0;
    while let Some(snap) = source.next_snapshot() {
        let mut graph = ArbitrageGraph::new();
        for pool in &snap.pools {
            for edge in pool.edges(probe_size) {
                graph.add_edge(edge);
            }
        }
        if let Some(cycle) = graph.find_arbitrage() {
            detected += 1;
            tracing::debug!(slot = snap.slot, product = cycle.product, "arb candidate");
            on_cycle(snap.slot, cycle);
        }
    }
    detected
}

#[cfg(test)]
mod tests {
    use super::*;
    use lorenz_core::types::{Bps, Dex};
    use lorenz_dex::CpmmPool;
    use replay::ReplaySource;

    fn pool(id: &str, a: &str, b: &str, ra: u128, rb: u128) -> Pool {
        Pool::Cpmm(CpmmPool {
            id: id.into(),
            dex: Dex::RaydiumAmm,
            token_a: a.into(),
            token_b: b.into(),
            reserve_a: ra,
            reserve_b: rb,
            fee: Bps(1),
        })
    }

    #[test]
    fn replay_source_feeds_detector_and_finds_arb() {
        // A snapshot with a SOL->USDC->BONK->SOL mispricing.
        let snap = PoolSnapshot {
            slot: 42,
            pools: vec![
                pool("p1", "SOL", "USDC", 1_000_000_000_000, 1_000_000_000_000),
                pool("p2", "USDC", "BONK", 1_000_000_000_000, 1_000_000_000_000),
                pool("p3", "BONK", "SOL", 1_000_000_000_000, 1_050_000_000_000),
            ],
        };
        let source = ReplaySource::new(vec![snap]);

        let mut slots = Vec::new();
        let n = detect_stream(source, 1_000_000, |slot, cycle| {
            assert!(cycle.product > 1.0);
            slots.push(slot);
        });
        assert_eq!(n, 1);
        assert_eq!(slots, vec![42]);
    }

    #[test]
    fn empty_source_detects_nothing() {
        let source = ReplaySource::new(vec![]);
        assert_eq!(detect_stream(source, 1_000, |_, _| {}), 0);
    }
}
