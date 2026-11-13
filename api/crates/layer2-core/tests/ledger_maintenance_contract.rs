//! Validate C-owned leaves before A registers the shared module exports.
extern crate layer3_adapter as production_adapter;
extern crate self as layer3_adapter;
pub use layer2_core::ids;
pub use production_adapter::valid_home_identity;

#[path = "../src/sandbox/agent_ledger_recovery.rs"]
pub mod agent_ledger_recovery;
#[path = "../src/sandbox/ledger_maintenance.rs"]
pub mod ledger_maintenance;
#[path = "../../layer3-adapter/src/runtime_ledger_maintenance.rs"]
pub mod runtime_ledger_maintenance;
