//! Mirror the owned module registration while reusing the real core domain types.
pub use layer2_core::sandbox::*;

#[path = "../../src/sandbox/lifecycle_test_support.rs"]
#[allow(dead_code)]
mod lifecycle_test_support;
#[path = "../../src/sandbox/runtime_update_test_support.rs"]
mod runtime_update_test_support;

#[path = "../../src/sandbox/agent_ledger_recovery.rs"]
pub mod agent_ledger_recovery;
#[path = "../../src/sandbox/agent_ledger_recovery_classification.rs"]
pub mod agent_ledger_recovery_classification;
#[path = "../../src/sandbox/ledger_maintenance.rs"]
pub mod ledger_maintenance;
#[path = "../../src/sandbox/ledger_maintenance_admission.rs"]
pub mod ledger_maintenance_admission;
#[path = "../../src/sandbox/ledger_maintenance_plan.rs"]
pub mod ledger_maintenance_plan;
