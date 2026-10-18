//! EconSim core: a monthly, stock-flow-consistent agent-based model of a US-like economy.
//! See docs/DESIGN.md for the full specification.

#[cfg(test)]
mod alloc_count;
pub mod banking;
pub mod common;
pub mod config;
pub mod consumption;
pub mod demography;
pub mod equity;
pub mod exog;
pub mod firms;
pub mod fiscal;
pub mod fx;
pub mod goods;
pub mod housing;
pub mod init;
pub mod labor;
pub mod ledger;
pub mod metrics;
pub mod monetary;
pub mod rng;
pub mod shocks;
pub mod sim;
pub mod start;
pub mod slab;
pub mod testkit;
pub mod types;
pub mod world;

pub use config::Config;
pub use metrics::{Metrics, Snapshot};
pub use shocks::{ScheduledShock, Shock};
pub use sim::{compare, run, step, Comparison, Scenario};
pub use types::World;
