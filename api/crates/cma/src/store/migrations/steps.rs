//! The explicit manifest registry is also the migrator's reader floor.
use super::{
    activity_indexes, activity_page_index, agent_log_segments, agent_owner_index,
    collection_indexes, opening_index, Result,
};
use mongodb::bson::{doc, Document};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Activity,
    ActivityPage,
    LedgerCollection,
    AgentOpening,
    AgentOwnerPage,
    AgentLogSegments,
}

impl Step {
    pub(super) const ALL: [Self; 6] = [
        Self::Activity,
        Self::ActivityPage,
        Self::LedgerCollection,
        Self::AgentOpening,
        Self::AgentOwnerPage,
        Self::AgentLogSegments,
    ];
    pub(super) fn parse(id: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|step| step.id() == id)
            .ok_or("unknown migration step")
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Activity => activity_indexes::ID,
            Self::ActivityPage => activity_page_index::ID,
            Self::LedgerCollection => collection_indexes::ID,
            Self::AgentOpening => opening_index::ID,
            Self::AgentOwnerPage => agent_owner_index::ID,
            Self::AgentLogSegments => agent_log_segments::ID,
        }
    }
    pub(super) fn checksum(self) -> String {
        match self {
            Self::Activity => activity_indexes::checksum(),
            Self::ActivityPage => activity_page_index::checksum(),
            Self::LedgerCollection => collection_indexes::checksum(),
            Self::AgentOpening => opening_index::checksum(),
            Self::AgentOwnerPage => agent_owner_index::checksum(),
            Self::AgentLogSegments => agent_log_segments::checksum(),
        }
    }
    pub(super) fn manifest(self) -> String {
        format!("manifest:{}", self.id())
    }
    pub(super) fn max_changes(self) -> i64 {
        match self {
            Self::Activity => 2,
            Self::ActivityPage => 1,
            Self::LedgerCollection => 5,
            Self::AgentOpening | Self::AgentOwnerPage => 1,
            Self::AgentLogSegments => agent_log_segments::MAX_SCHEMA_STEPS,
        }
    }
    pub(super) fn collections(self) -> &'static [&'static str] {
        match self {
            Self::Activity | Self::ActivityPage => &["activity"],
            Self::LedgerCollection => &collection_indexes::COLLECTIONS,
            Self::AgentOpening => &opening_index::COLLECTIONS,
            Self::AgentOwnerPage => &["agents"],
            Self::AgentLogSegments => crate::store::agent_log_segments::COLLECTIONS,
        }
    }
    pub(super) fn effect_field(self) -> &'static str {
        match self {
            Self::AgentLogSegments => "applied_schema_steps",
            Self::Activity
            | Self::ActivityPage
            | Self::LedgerCollection
            | Self::AgentOpening
            | Self::AgentOwnerPage => "created_indexes",
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
            Self::ActivityPage => Ok(activity_page_index::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("activity", index))
                .collect()),
            Self::LedgerCollection => collection_indexes::inspect(database).await,
            Self::AgentOpening => opening_index::inspect(database).await,
            Self::AgentOwnerPage => Ok(agent_owner_index::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("agents", index))
                .collect()),
            Self::AgentLogSegments => Err("segment migration requires schema-unit inspection"),
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
            Self::ActivityPage if collection == "activity" => {
                activity_page_index::create(database, index).await
            }
            Self::LedgerCollection => collection_indexes::create(database, collection, index).await,
            Self::AgentOpening => opening_index::create(database, collection, index).await,
            Self::AgentOwnerPage if collection == "agents" => {
                agent_owner_index::create(database, index).await
            }
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
