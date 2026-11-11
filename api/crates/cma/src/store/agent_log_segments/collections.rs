use crate::store::{durability::majority_collection, sandbox_mongo::MongoSandboxStore};
use mongodb::{bson::Document, Collection, Database};

/// Handles use the same client/database as the original Agent collection.
/// Constructing them performs no DDL, preparation or configuration mutation.
pub(super) fn collection(store: &MongoSandboxStore, name: &str) -> Collection<Document> {
    match name {
        super::HEADS => store.segment_logs.heads.clone(),
        super::SEGMENTS => store.segment_logs.seals.clone(),
        super::PUBLICATIONS => store.segment_logs.publications.clone(),
        super::PROJECTIONS => store.segment_logs.projections.clone(),
        super::EVIDENCE => store.segment_logs.evidence.clone(),
        super::AUDITS => store.segment_logs.audits.clone(),
        _ => majority_collection(&store.client.database(&store.agents.namespace().db), name),
    }
}

/// Handles only. Construction never creates collections or changes validators.
#[derive(Clone)]
pub(in crate::store) struct SegmentCollections {
    pub(in crate::store) heads: Collection<Document>,
    pub(in crate::store) seals: Collection<Document>,
    pub(in crate::store) publications: Collection<Document>,
    pub(in crate::store) projections: Collection<Document>,
    pub(in crate::store) evidence: Collection<Document>,
    pub(in crate::store) audits: Collection<Document>,
}

impl SegmentCollections {
    pub(in crate::store) fn new(database: &Database) -> Self {
        Self {
            heads: majority_collection(database, super::HEADS),
            seals: majority_collection(database, super::SEGMENTS),
            publications: majority_collection(database, super::PUBLICATIONS),
            projections: majority_collection(database, super::PROJECTIONS),
            evidence: majority_collection(database, super::EVIDENCE),
            audits: majority_collection(database, super::AUDITS),
        }
    }
}
