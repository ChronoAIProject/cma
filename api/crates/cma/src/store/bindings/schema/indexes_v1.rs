//! Immutable binding index manifest. Semantic changes require a new version.
use mongodb::bson::{doc, Document};

const BINDINGS: &str = "provider_bindings";
const CONVERSATIONS: &str = "binding_conversations";
const PREPARATIONS: &str = "provider_binding_preparations";
pub(in crate::store) const COLLECTIONS: &[&str] = &[
    BINDINGS,
    CONVERSATIONS,
    "provider_tool_opens",
    "provider_tool_relays",
    PREPARATIONS,
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
            doc! {"name":"owner_agent_association", "key":doc! {"record.owner":1,"record.agent_id":1,"_id":1}},
            // Agent reuse is unique within a binding generation. The legacy
            // global constraint can only be retired by a separate cutover.
            doc! {"name":"binding_generation_agent", "key":{"binding_id":1,"generation":1,"record.agent_id":1}, "unique":true},
        ],
        PREPARATIONS => vec![
            doc! {"name":"binding_id_1_generation_1", "key":{"binding_id":1,"generation":1}, "unique":true},
        ],
        _ => Vec::new(),
    }
}

pub(in crate::store) fn missing(
    collection: &str,
    existing: &[Document],
) -> Result<Vec<Document>, &'static str> {
    if !COLLECTIONS.contains(&collection) {
        return Err("unknown binding collection");
    }
    let mut pending = Vec::new();
    for desired in indexes(collection) {
        pending.extend(crate::store::schema::index_contract_v1::missing(
            existing, &desired,
        )?);
    }
    Ok(pending)
}
