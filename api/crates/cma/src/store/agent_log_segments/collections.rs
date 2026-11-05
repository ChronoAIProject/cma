use crate::store::durability::majority_collection;
use mongodb::{bson::Document, Collection, Database};

#[derive(Clone)]
pub(in crate::store) struct SegmentCollections {
    pub heads: Collection<Document>,
    pub seals: Collection<Document>,
    pub audits: Collection<Document>,
    pub projections: Collection<Document>,
    pub publications: Collection<Document>,
    pub evidence: Collection<Document>,
}

impl SegmentCollections {
    pub(in crate::store) fn open(database: &Database) -> Self {
        Self {
            heads: majority_collection(database, super::HEADS),
            seals: majority_collection(database, super::SEGMENTS),
            audits: majority_collection(database, super::AUDITS),
            projections: majority_collection(database, super::PROJECTIONS),
            publications: majority_collection(database, super::PUBLICATIONS),
            evidence: majority_collection(database, super::EVIDENCE),
        }
    }
}
