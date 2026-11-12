//! The explicit manifest registry is also the migrator's reader floor.
use super::{activity_indexes, collection_indexes, Result};
use mongodb::bson::{doc, Document};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Activity,
    LedgerCollection,
}

impl Step {
    pub(super) const ALL: [Self; 2] = [Self::Activity, Self::LedgerCollection];
    pub(super) fn parse(id: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|step| step.id() == id)
            .ok_or("unknown migration step")
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Activity => activity_indexes::ID,
            Self::LedgerCollection => collection_indexes::ID,
        }
    }
    pub(super) fn checksum(self) -> String {
        match self {
            Self::Activity => activity_indexes::checksum(),
            Self::LedgerCollection => collection_indexes::checksum(),
        }
    }
    pub(super) fn manifest(self) -> String {
        format!("manifest:{}", self.id())
    }
    pub(super) fn max_changes(self) -> i64 {
        match self {
            Self::Activity => 2,
            Self::LedgerCollection => 5,
        }
    }
    pub(super) fn collections(self) -> &'static [&'static str] {
        match self {
            Self::Activity => &["activity"],
            Self::LedgerCollection => &collection_indexes::COLLECTIONS,
        }
    }
    pub(super) async fn inspect(
        self,
        database: &mongodb::Database,
    ) -> Result<Vec<(&'static str, Document)>> {
        match self {
            Self::Activity => Ok(activity_indexes::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("activity", index))
                .collect()),
            Self::LedgerCollection => collection_indexes::inspect(database).await,
        }
    }
    pub(super) async fn create(
        self,
        database: &mongodb::Database,
        collection: &str,
        index: Document,
    ) -> Result<bool> {
        match self {
            Self::Activity if collection == "activity" => {
                activity_indexes::create(database, index).await
            }
            Self::LedgerCollection => collection_indexes::create(database, collection, index).await,
            _ => Err("migration collection mismatch"),
        }
    }
}

pub(super) fn unsupported() -> Document {
    let allowed: Vec<_> = Step::ALL.into_iter().flat_map(|step| [
        doc! {"kind":"manifest","_id":step.manifest(),"migration":step.id(),"checksum":step.checksum()},
        doc! {"kind":"attempt","migration":step.id(),"checksum":step.checksum()},
    ]).chain([doc! {"kind":"lease","_id":"runner"}]).collect();
    doc! {"$or":[
        {"schema":{"$ne":1_i64}}, {"schema":{"$not":{"$type":["int","long"]}}},
        {"schema":{"$type":"array"}}, {"kind":{"$not":{"$type":"string"}}},
        {"kind":{"$type":"array"}}, {"$nor":allowed}
    ]}
}
