//! Deterministic replay source over recorded snapshots.

use crate::{PoolSnapshot, PoolSnapshotSource};
use std::collections::VecDeque;

/// Replays a fixed sequence of snapshots in order. Fully deterministic: useful
/// for tests, backtests, and reproducing a recorded market window.
#[derive(Debug, Clone)]
pub struct ReplaySource {
    queue: VecDeque<PoolSnapshot>,
}

impl ReplaySource {
    pub fn new(snapshots: Vec<PoolSnapshot>) -> Self {
        Self {
            queue: snapshots.into(),
        }
    }

    pub fn remaining(&self) -> usize {
        self.queue.len()
    }
}

impl PoolSnapshotSource for ReplaySource {
    fn next_snapshot(&mut self) -> Option<PoolSnapshot> {
        self.queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yields_in_order_then_exhausts() {
        let mk = |slot| PoolSnapshot {
            slot,
            pools: vec![],
        };
        let mut s = ReplaySource::new(vec![mk(1), mk(2)]);
        assert_eq!(s.remaining(), 2);
        assert_eq!(s.next_snapshot().unwrap().slot, 1);
        assert_eq!(s.next_snapshot().unwrap().slot, 2);
        assert!(s.next_snapshot().is_none());
    }
}
