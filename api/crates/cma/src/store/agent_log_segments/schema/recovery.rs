//! The frozen S5 carrier shape is shared with its typed codec and discovery.
use mongodb::{bson::Document, IndexModel};
use std::sync::LazyLock;

pub(super) fn validator() -> Document {
    static DEFINITION: LazyLock<Document> = LazyLock::new(|| {
        serde_json::from_str(include_str!(
            "../../agent_capacity_recovery/format2.schema.json"
        ))
        .expect("the frozen recovery validator must be valid BSON-compatible JSON")
    });
    DEFINITION.clone()
}

pub(super) fn indexes() -> Vec<IndexModel> {
    static DEFINITIONS: LazyLock<Vec<IndexModel>> = LazyLock::new(|| {
        // Deserialize directly: preserve the authored compound-key order and
        // exact integer bounds without an intermediate unordered JSON map.
        serde_json::from_str(include_str!(
            "../../agent_capacity_recovery/format2.indexes.json"
        ))
        .expect("the frozen recovery indexes must be valid index models")
    });
    DEFINITIONS.clone()
}
