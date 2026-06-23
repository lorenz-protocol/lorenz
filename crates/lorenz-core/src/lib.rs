//! `lorenz-core` holds the vocabulary shared across the whole platform: domain
//! newtypes, the typed configuration model, the error type and observability
//! bootstrap.
//!
//! Design rule: this crate must stay free of any I/O, RPC client or on-chain
//! dependency. It is the deterministic vocabulary that both the data plane and
//! the control plane agree on, which is what makes traces replayable.

pub mod config;
pub mod error;
pub mod telemetry;
pub mod types;

pub use error::{Error, Result};
