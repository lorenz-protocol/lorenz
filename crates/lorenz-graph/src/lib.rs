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

    /// Find up to `max_cycles` profitable cycles that are pairwise
    /// **edge-disjoint**: no two returned cycles share the same directed edge,
    /// where an edge's identity is its `(from, to, pool, rate)` value.
    ///
    /// The search reuses the exact negative-cycle logic of [`find_arbitrage`]:
    /// it finds one profitable cycle over a working copy of the edge set,
    /// records it, removes that cycle's edges from the working set (so a later
    /// cycle cannot reuse any of them), and repeats until no profitable cycle
    /// remains or `max_cycles` is reached. Because every iteration removes at
    /// least one edge, the loop is guaranteed to terminate (at most
    /// `edge_count()` iterations).
    ///
    /// Determinism: the working set is a `Vec` seeded from insertion order and
    /// only ever has elements removed in place, so ordering never depends on
    /// `HashMap` iteration order. Given the same graph, the returned cycles and
    /// their order are identical across runs.
    ///
    /// `max_cycles == 0` returns an empty vector. The result length is always
    /// `<= max_cycles` and `<= edge_count()`.
    ///
    /// [`find_arbitrage`]: Self::find_arbitrage
    pub fn find_disjoint_arbitrage(&self, max_cycles: usize) -> Vec<Cycle> {
        // Removal strategy: drop exactly the cycle's own directed edges (each
        // matched once, by `(from, to, pool, rate)` value), which is what makes
        // subsequent cycles edge-disjoint from this one.
        self.accumulate_disjoint(max_cycles, |working, cycle| {
            let mut removed = 0usize;
            for ce in &cycle.edges {
                if let Some(pos) = working.iter().position(|e| e == ce) {
                    working.remove(pos);
                    removed += 1;
                }
            }
            removed
        })
    }

    /// Find up to `max_cycles` profitable cycles that are pairwise
    /// **pool-disjoint**: no two returned cycles share any `PoolId`.
    ///
    /// This is a strictly stronger guarantee than [`find_disjoint_arbitrage`],
    /// which only rules out shared *directed edges*. Two cycles can be
    /// edge-disjoint yet still touch the same pool (a pool contributes edges in
    /// both directions, and a multi-asset pool contributes edges among several
    /// pairs). Such cycles would compete on that pool's reserves at execution,
    /// so they cannot be sized independently. Requiring pool-disjointness makes
    /// the returned opportunities independently executable.
    ///
    /// The loop mirrors [`find_disjoint_arbitrage`]: it finds one profitable
    /// cycle over a working copy of the edge set, records it, then prunes the
    /// working set. The only difference is the pruning rule — after accepting a
    /// cycle we collect the set of `PoolId`s it uses and retain only edges whose
    /// `pool` is not in that set, removing *every* edge on any of those pools
    /// (not just the cycle's own edges). This is what forbids a later cycle from
    /// reusing any of those pools.
    ///
    /// Termination: an accepted cycle's edges are all present in the working
    /// set, so their pools are too; retaining "pool not used" therefore removes
    /// at least the cycle's own edges (>= 1). A defensive break also stops the
    /// loop if a prune ever removes nothing, keeping termination unconditional
    /// (at most `edge_count()` iterations).
    ///
    /// Determinism: the working set is a `Vec` seeded from insertion order and
    /// only ever has elements removed in place (`retain` preserves order), so
    /// output never depends on `HashMap` iteration order. Given the same graph,
    /// the returned cycles and their order are identical across runs.
    ///
    /// `max_cycles == 0` returns an empty vector. The result length is always
    /// `<= max_cycles` and `<= edge_count()`.
    ///
    /// [`find_arbitrage`]: Self::find_arbitrage
    /// [`find_disjoint_arbitrage`]: Self::find_disjoint_arbitrage
    pub fn find_pool_disjoint_arbitrage(&self, max_cycles: usize) -> Vec<Cycle> {
        // Removal strategy: drop every edge whose pool is used anywhere by the
        // accepted cycle, which is what makes subsequent cycles pool-disjoint
        // from this one.
        self.accumulate_disjoint(max_cycles, |working, cycle| {
            let pools: Vec<PoolId> = cycle.edges.iter().map(|e| e.pool.clone()).collect();
            let before = working.len();
            working.retain(|e| !pools.contains(&e.pool));
            before - working.len()
        })
    }

    /// Shared driver for the disjoint-cycle searches.
    ///
    /// Repeatedly finds one profitable cycle over a working copy of the edge
    /// set (reusing the exact [`find_arbitrage`] Bellman-Ford search), records
    /// it, and then applies `prune` to remove edges from the working set before
    /// searching again. `prune` returns how many edges it removed; if it ever
    /// removes zero the loop breaks, guaranteeing forward progress and
    /// termination regardless of the strategy.
    ///
    /// [`find_arbitrage`]: Self::find_arbitrage
    fn accumulate_disjoint<F>(&self, max_cycles: usize, mut prune: F) -> Vec<Cycle>
    where
        F: FnMut(&mut Vec<Edge>, &Cycle) -> usize,
    {
        let mut result = Vec::new();
        if max_cycles == 0 {
            return result;
        }

        // Working copy of the edge set, in deterministic insertion order.
        let mut working: Vec<Edge> = self.edges.clone();

        while result.len() < max_cycles {
            // Rebuild a graph over the remaining edges and reuse the identical
            // Bellman-Ford negative-cycle search.
            let mut graph = ArbitrageGraph::new();
            for e in &working {
                graph.add_edge(e.clone());
            }

            let Some(cycle) = graph.find_arbitrage() else {
                break;
            };

            // Prune per the caller's strategy. A cycle's edges came from
            // `working`, so a correct strategy removes >= 1; a zero return is
            // treated as no progress and stops the loop.
            let removed = prune(&mut working, &cycle);
            if removed == 0 {
                break;
            }

            result.push(cycle);
        }

        result
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

    /// True when the summed `-ln(rate)` weight of the cycle is negative, i.e.
    /// the product of rates exceeds 1 (a genuinely profitable loop).
    fn is_profitable(cycle: &Cycle) -> bool {
        let weight: f64 = cycle.edges.iter().map(|e| -e.rate.ln()).sum();
        weight < 0.0
    }

    fn share_an_edge(a: &Cycle, b: &Cycle) -> bool {
        a.edges.iter().any(|ea| b.edges.iter().any(|eb| ea == eb))
    }

    fn share_a_pool(a: &Cycle, b: &Cycle) -> bool {
        a.edges
            .iter()
            .any(|ea| b.edges.iter().any(|eb| ea.pool == eb.pool))
    }

    /// A graph holding two independent triangular opportunities on disjoint
    /// token sets, plus one balanced (non-profitable) pair.
    fn two_disjoint_opportunities() -> ArbitrageGraph {
        let mut g = ArbitrageGraph::new();
        // Triangle 1: A -> B -> C -> A, product 1.1 > 1.
        g.add_edge(edge("A", "B", "p1", 1.0));
        g.add_edge(edge("B", "C", "p2", 1.0));
        g.add_edge(edge("C", "A", "p3", 1.1));
        // Triangle 2: D -> E -> F -> D, product 1.2 > 1.
        g.add_edge(edge("D", "E", "p4", 1.0));
        g.add_edge(edge("E", "F", "p5", 1.0));
        g.add_edge(edge("F", "D", "p6", 1.2));
        // Balanced pair, contributes no profitable cycle.
        g.add_edge(edge("G", "H", "p7", 2.0));
        g.add_edge(edge("H", "G", "p7", 0.5));
        g
    }

    #[test]
    fn disjoint_zero_cap_returns_empty() {
        let g = two_disjoint_opportunities();
        assert!(g.find_disjoint_arbitrage(0).is_empty());
    }

    #[test]
    fn disjoint_no_opportunity_returns_empty() {
        let mut g = ArbitrageGraph::new();
        g.add_edge(edge("A", "B", "p1", 2.0));
        g.add_edge(edge("B", "A", "p1", 0.5)); // product = 1
        assert!(g.find_disjoint_arbitrage(8).is_empty());
    }

    #[test]
    fn disjoint_single_opportunity_returns_exactly_one() {
        let mut g = ArbitrageGraph::new();
        g.add_edge(edge("A", "B", "p1", 1.0));
        g.add_edge(edge("B", "C", "p2", 1.0));
        g.add_edge(edge("C", "A", "p3", 1.1));

        let cycles = g.find_disjoint_arbitrage(8);
        assert_eq!(cycles.len(), 1);
        assert!(is_profitable(&cycles[0]));
        assert!(cycles[0].product > 1.0);
    }

    #[test]
    fn disjoint_finds_both_and_they_are_edge_disjoint() {
        let g = two_disjoint_opportunities();
        let cycles = g.find_disjoint_arbitrage(8);

        assert_eq!(cycles.len(), 2);

        // Every returned cycle is genuinely profitable.
        for c in &cycles {
            assert!(is_profitable(c), "product was {}", c.product);
            assert!(c.product > 1.0);
        }

        // Pairwise edge-disjoint.
        for (i, ci) in cycles.iter().enumerate() {
            for cj in &cycles[i + 1..] {
                assert!(!share_an_edge(ci, cj));
            }
        }

        // Bounds hold: <= max_cycles and <= initial edge count.
        assert!(cycles.len() <= 8);
        assert!(cycles.len() <= g.edge_count());
    }

    #[test]
    fn disjoint_respects_max_cycles_cap() {
        let g = two_disjoint_opportunities();
        let cycles = g.find_disjoint_arbitrage(1);
        assert_eq!(cycles.len(), 1);
        assert!(is_profitable(&cycles[0]));
    }

    #[test]
    fn disjoint_output_is_deterministic() {
        let g = two_disjoint_opportunities();
        let a = g.find_disjoint_arbitrage(8);
        let b = g.find_disjoint_arbitrage(8);
        assert_eq!(a, b);
    }

    /// Two profitable triangles on disjoint token sets that nonetheless share a
    /// single pool: each triangle's closing edge sits on `pShared` (as a
    /// multi-asset pool would, contributing edges between different pairs). The
    /// triangles are edge-disjoint (every directed edge is distinct) but not
    /// pool-disjoint.
    fn two_edge_disjoint_but_pool_sharing() -> ArbitrageGraph {
        let mut g = ArbitrageGraph::new();
        // Triangle 1: A -> B -> C -> A, product 1.1 > 1. Closes on `pShared`.
        g.add_edge(edge("A", "B", "p1", 1.0));
        g.add_edge(edge("B", "C", "p2", 1.0));
        g.add_edge(edge("C", "A", "pShared", 1.1));
        // Triangle 2: D -> E -> F -> D, product 1.2 > 1. Also closes on
        // `pShared`, so the two triangles collide on that pool's reserves.
        g.add_edge(edge("D", "E", "p4", 1.0));
        g.add_edge(edge("E", "F", "p5", 1.0));
        g.add_edge(edge("F", "D", "pShared", 1.2));
        g
    }

    #[test]
    fn pool_disjoint_zero_cap_returns_empty() {
        let g = two_disjoint_opportunities();
        assert!(g.find_pool_disjoint_arbitrage(0).is_empty());
    }

    #[test]
    fn pool_disjoint_no_opportunity_returns_empty() {
        let mut g = ArbitrageGraph::new();
        g.add_edge(edge("A", "B", "p1", 2.0));
        g.add_edge(edge("B", "A", "p1", 0.5)); // product = 1
        assert!(g.find_pool_disjoint_arbitrage(8).is_empty());
    }

    #[test]
    fn pool_disjoint_single_opportunity_returns_exactly_one() {
        let mut g = ArbitrageGraph::new();
        g.add_edge(edge("A", "B", "p1", 1.0));
        g.add_edge(edge("B", "C", "p2", 1.0));
        g.add_edge(edge("C", "A", "p3", 1.1));

        let cycles = g.find_pool_disjoint_arbitrage(8);
        assert_eq!(cycles.len(), 1);
        assert!(is_profitable(&cycles[0]));
        assert!(cycles[0].product > 1.0);
    }

    #[test]
    fn pool_disjoint_finds_both_when_pools_distinct() {
        // Distinct pools per edge, so pool-disjointness coincides with
        // edge-disjointness here: both opportunities are returned.
        let g = two_disjoint_opportunities();
        let cycles = g.find_pool_disjoint_arbitrage(8);

        assert_eq!(cycles.len(), 2);

        for c in &cycles {
            assert!(is_profitable(c), "product was {}", c.product);
            assert!(c.product > 1.0);
        }

        // Pairwise pool-disjoint (and therefore also edge-disjoint).
        for (i, ci) in cycles.iter().enumerate() {
            for cj in &cycles[i + 1..] {
                assert!(!share_a_pool(ci, cj));
                assert!(!share_an_edge(ci, cj));
            }
        }

        assert!(cycles.len() <= 8);
        assert!(cycles.len() <= g.edge_count());
    }

    #[test]
    fn pool_disjoint_respects_max_cycles_cap() {
        let g = two_disjoint_opportunities();
        let cycles = g.find_pool_disjoint_arbitrage(1);
        assert_eq!(cycles.len(), 1);
        assert!(is_profitable(&cycles[0]));
    }

    #[test]
    fn pool_disjoint_output_is_deterministic() {
        let g = two_disjoint_opportunities();
        let a = g.find_pool_disjoint_arbitrage(8);
        let b = g.find_pool_disjoint_arbitrage(8);
        assert_eq!(a, b);

        let g2 = two_edge_disjoint_but_pool_sharing();
        assert_eq!(
            g2.find_pool_disjoint_arbitrage(8),
            g2.find_pool_disjoint_arbitrage(8)
        );
    }

    #[test]
    fn pool_sharing_yields_one_where_edge_disjoint_yields_two() {
        let g = two_edge_disjoint_but_pool_sharing();

        // The edge-disjoint search sees two independent opportunities: the two
        // triangles share no directed edge.
        let edge_disjoint = g.find_disjoint_arbitrage(8);
        assert_eq!(edge_disjoint.len(), 2);
        // They do, however, share a pool -- so they are NOT pool-disjoint.
        assert!(share_a_pool(&edge_disjoint[0], &edge_disjoint[1]));

        // The pool-disjoint search rejects the second: accepting one triangle
        // removes every edge on `pShared`, which severs the other triangle.
        let pool_disjoint = g.find_pool_disjoint_arbitrage(8);
        assert_eq!(pool_disjoint.len(), 1);
        assert!(is_profitable(&pool_disjoint[0]));
        assert!(pool_disjoint[0].product > 1.0);
    }
}
