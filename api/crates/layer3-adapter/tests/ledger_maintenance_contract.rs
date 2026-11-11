//! Compile the owned leaf before the integration owner registers its public module.
extern crate layer3_adapter as production_adapter;
extern crate self as layer3_adapter;
pub use production_adapter::valid_home_identity;

#[path = "../../layer4-runtime/src/retirement_activation.rs"]
pub mod retirement_activation;
#[path = "../src/runtime_ledger_maintenance.rs"]
pub mod runtime_ledger_maintenance;

#[test]
fn exclusion_can_move_with_detached_storage_work() {
    fn assert_send<T: Send>() {}
    assert_send::<retirement_activation::ValidatedLedgerExclusion>();
}
