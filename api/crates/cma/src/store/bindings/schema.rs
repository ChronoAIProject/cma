//! The binding index contract shared by startup inspection and explicit migration.

use mongodb::bson::{doc, Document};

use super::{associations, reprovision, BINDINGS, CONVERSATIONS, TOOL_OPENS, TOOL_RELAYS};

pub(in crate::store) const COLLECTIONS: &[&str] = &[
    BINDINGS,
    CONVERSATIONS,
    TOOL_OPENS,
    TOOL_RELAYS,
    reprovision::PREPARATIONS,
];
pub(in crate::store) const LEGACY_AGENT_INDEX: &str = "record.agent_id_1";

pub(in crate::store) fn indexes(collection: &str) -> Vec<Document> {
    match collection {
        BINDINGS => vec![
            doc! {"name":"pending_1__id_1", "key":{"pending":1,"_id":1}},
            doc! {"name":"record.state.status_1_record.work_1__id_1", "key":{"record.state.status":1,"record.work":1,"_id":1}},
            doc! {"name":"key_history_1", "key":{"key_history":1}},
            doc! {"name":"record.owner_1__id_1", "key":{"record.owner":1,"_id":1}},
            doc! {"name":"record.key_sha256_1", "key":{"record.key_sha256":1}, "unique":true},
        ],
        CONVERSATIONS => vec![
            doc! {"name":"pending_1__id_1", "key":{"pending":1,"_id":1}},
            doc! {"name":"record.binding_id_1", "key":{"record.binding_id":1}},
            doc! {"name":"binding_id_1_generation_1_conversation_id_1", "key":{"binding_id":1,"generation":1,"conversation_id":1}},
            doc! {"name":associations::INDEX, "key":associations::index_keys()},
            // Agent reuse is unique within a binding generation. The legacy
            // global constraint can only be retired by a separate cutover.
            doc! {"name":"binding_generation_agent", "key":{"binding_id":1,"generation":1,"record.agent_id":1}, "unique":true},
        ],
        reprovision::PREPARATIONS => vec![
            doc! {"name":"binding_id_1_generation_1", "key":{"binding_id":1,"generation":1}, "unique":true},
        ],
        _ => Vec::new(),
    }
}
