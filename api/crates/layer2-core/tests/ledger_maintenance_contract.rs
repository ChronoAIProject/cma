//! Validate C-owned leaves before A registers the shared module exports.
extern crate layer3_adapter as production_adapter;
extern crate self as layer3_adapter;
pub use layer2_core::*;
pub use production_adapter::{
    agent_sandbox, valid_home_identity, CheckpointReceipt, ConversationSnapshot, HomeVolumeMode,
    HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope, RuntimeImage, WriterState,
    RUNTIME_CONTRACT_LABEL,
};

#[path = "../../layer3-adapter/src/runtime_ledger_maintenance.rs"]
pub mod runtime_ledger_maintenance;
#[path = "ledger_contract/mod.rs"]
pub mod sandbox;
