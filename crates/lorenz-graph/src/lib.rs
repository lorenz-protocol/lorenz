//! Arbitrage detection as a negative-cycle search.
//!
//! Model the market as a directed graph: nodes are tokens, an edge `A -> B`
//! means "you can swap A into B" with some effective rate `r` (out per in).
//! Chaining swaps multiplies rates, so a cycle is profitable exactly when the
//! product of its rates exceeds 1.
//!
//! Take `-ln(r)` as the edge weight and the product condition becomes an
//! additive one: a profitable cycle is a cycle whose summed weights are
//! negative. That is the classic Bellman-Ford negative-cycle problem, which we
//! solve and then reconstruct the offending cycle from.
//!
//! Caveat (stated honestly): edge rates here are taken at a single quote size,
//! so this finds *candidate* cycles. Exact, size-aware profit is recomputed by
//! the executor/backtester against real pool math (`lorenz-amm`) before anything
//! is acted on. This module narrows the search space; it does not certify
//! profit.

use lorenz_core::types::{PoolId, TokenId};
use std::collections::HashMap;

/// A directed swap edge with its effective rate (output per unit input).
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: TokenId,
    pub to: TokenId,
    pub pool: PoolId,
    /// Effective rate out/in for the quote size this edge was built at.
    pub rate: f64,
}

/// A detected profitable cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct Cycle {
    /// Edges in traversal order; the last edge's `to` equals the first's `from`.
    pub edges: Vec<Edge>,
    /// Product of rates around the loop. `> 1.0` means profitable (gross).
    pub product: f64,
}

/// Detector that owns the token index and adjacency list.
#[derive(Debug, Default)]
pub struct ArbitrageGraph {
    edges: Vec<Edge>,
    index: HashMap<TokenId, usize>,
    tokens: Vec<TokenId>,
}

impl ArbitrageGraph {
    pub fn new() -> Self {
        Self::default()
    }

    fn intern(&mut self, t: &TokenId) -> usize {
        if let Some(&i) = self.index.get(t) {
            return i;
        }
        let i = self.tokens.len();
        self.tokens.push(t.clone());
        self.index.insert(t.clone(), i);
        i
    }

    /// Add a directed edge. Edges with non-positive rate are ignored (no path).
    pub fn add_edge(&mut self, edge: Edge) {
        if edge.rate <= 0.0 || !edge.rate.is_finite() {
            return;
        }
        self.intern(&edge.from);
        self.intern(&edge.to);
        self.edges.push(edge);
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Find one profitable cycle, if any, via Bellman-Ford.
    ///
    /// All distances start at 0, which is equivalent to attaching a virtual
    /// source with zero-weight edges to every node; this lets us detect a
    /// negative cycle anywhere in the graph regardless of connectivity.
    pub fn find_arbitrage(&self) -> Option<Cycle> {
        let n = self.tokens.len();
        if n == 0 || self.edges.is_empty() {
            return None;
        }

        let weights: Vec<f64> = self.edges.iter().map(|e| -e.rate.ln()).collect();
        let mut dist = vec![0.0f64; n];
        // pred[v] = index of the edge used to reach v.
        let mut pred_edge = vec![usize::MAX; n];

        let idx = |t: &TokenId| *self.index.get(t).unwrap();
        let mut last_updated: Option<usize> = None;

        for _ in 0..n {
            last_updated = None;
            for (ei, e) in self.edges.iter().enumerate() {
                let u = idx(&e.from);
                let v = idx(&e.to);
                if dist[u] + weights[ei] < dist[v] - 1e-12 {
                    dist[v] = dist[u] + weights[ei];
                    pred_edge[v] = ei;
                    last_updated = Some(v);
                }
            }
            // Converged with no negative cycle: nothing relaxed this round.
            last_updated?;
        }

        // A node still relaxing after `n` rounds sits on / reaches a negative
        // cycle. Walk predecessors `n` times to step into the cycle proper.
        let mut v = last_updated?;
        for _ in 0..n {
            let ei = pred_edge[v];
            if ei == usize::MAX {
                return None;
            }
            v = idx(&self.edges[ei].from);
        }

        // Reconstruct the cycle by following predecessor edges from `v` back to
        // `v`.
        let start = v;
        let mut cycle_edges = Vec::new();
        let mut cur = start;
        loop {
            let ei = pred_edge[cur];
            if ei == usize::MAX {
                return None;
            }
            cycle_edges.push(self.edges[ei].clone());
            cur = idx(&self.edges[ei].from);
            if cur == start {
                break;
            }
            if cycle_edges.len() > n + 1 {
                // Safety valve against pathological reconstruction.
                return None;
            }
        }
        cycle_edges.reverse();

        let product: f64 = cycle_edges.iter().map(|e| e.rate).product();
        Some(Cycle {
            edges: cycle_edges,
            product,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, pool: &str, rate: f64) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            pool: pool.into(),
            rate,
        }
    }

    #[test]
    fn no_arbitrage_when_rates_balanced() {
        let mut g = ArbitrageGraph::new();
        g.add_edge(edge("A", "B", "p1", 2.0));
        g.add_edge(edge("B", "A", "p1", 0.5)); // exact inverse, product = 1
        assert!(g.find_arbitrage().is_none());
    }

    #[test]
    fn finds_triangular_arbitrage() {
        let mut g = ArbitrageGraph::new();
        // A -> B -> C -> A with product 1 * 1 * 1.1 = 1.1 (profitable).
        g.add_edge(edge("A", "B", "p1", 1.0));
        g.add_edge(edge("B", "C", "p2", 1.0));
        g.add_edge(edge("C", "A", "p3", 1.1));

        let cycle = g.find_arbitrage().expect("should find an arb cycle");
        assert!(cycle.product > 1.0, "product was {}", cycle.product);
        assert_eq!(cycle.edges.len(), 3);

        // The reconstructed edges must form a closed loop.
        for win in cycle.edges.windows(2) {
            assert_eq!(win[0].to, win[1].from);
        }
        let first = &cycle.edges[0];
        let last = &cycle.edges[cycle.edges.len() - 1];
        assert_eq!(last.to, first.from);
    }

    #[test]
    fn ignores_non_positive_rates() {
        let mut g = ArbitrageGraph::new();
        g.add_edge(edge("A", "B", "p1", 0.0));
        g.add_edge(edge("A", "B", "p2", -1.0));
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn empty_graph_has_no_arbitrage() {
        let g = ArbitrageGraph::new();
        assert!(g.find_arbitrage().is_none());
    }
}
