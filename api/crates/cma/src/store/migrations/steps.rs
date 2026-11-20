//! The explicit manifest registry is also the migrator's reader floor.
use super::{
    activity_indexes, activity_page_index, agent_log_segments, agent_log_source_keys,
    agent_owner_index, binding_indexes, collection_indexes, command_cohort_index, opening_index,
    Result,
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
    AgentLogSourceKeys,
    BindingIndexes,
    CommandCohort,
}

impl Step {
    pub(super) const ALL: [Self; 9] = [
        Self::Activity,
        Self::ActivityPage,
        Self::LedgerCollection,
        Self::AgentOpening,
        Self::AgentOwnerPage,
        Self::AgentLogSegments,
        Self::AgentLogSourceKeys,
        Self::BindingIndexes,
        Self::CommandCohort,
    ];
    pub(super) fn parse(id: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|step| step.id() == id)
            .ok_or("unknown migration step")
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::CommandCohort => command_cohort_index::ID,
            Self::BindingIndexes => binding_indexes::ID,
            Self::Activity => activity_indexes::ID,
            Self::ActivityPage => activity_page_index::ID,
            Self::LedgerCollection => collection_indexes::ID,
            Self::AgentOpening => opening_index::ID,
            Self::AgentOwnerPage => agent_owner_index::ID,
            Self::AgentLogSegments => agent_log_segments::ID,
            Self::AgentLogSourceKeys => agent_log_source_keys::ID,
        }
    }
    pub(super) fn checksum(self) -> String {
        match self {
            Self::CommandCohort => command_cohort_index::checksum(),
            Self::BindingIndexes => binding_indexes::checksum(),
            Self::Activity => activity_indexes::checksum(),
            Self::ActivityPage => activity_page_index::checksum(),
            Self::LedgerCollection => collection_indexes::checksum(),
            Self::AgentOpening => opening_index::checksum(),
            Self::AgentOwnerPage => agent_owner_index::checksum(),
            Self::AgentLogSegments => agent_log_segments::checksum(),
            Self::AgentLogSourceKeys => agent_log_source_keys::checksum(),
        }
    }
    pub(super) fn manifest(self) -> String {
        format!("manifest:{}", self.id())
    }
    pub(super) fn max_changes(self) -> i64 {
        match self {
            Self::CommandCohort => 1,
            Self::BindingIndexes => 11,
            Self::Activity => 2,
            Self::ActivityPage => 1,
            Self::LedgerCollection => 5,
            Self::AgentOpening | Self::AgentOwnerPage | Self::AgentLogSourceKeys => 1,
            Self::AgentLogSegments => agent_log_segments::MAX_SCHEMA_STEPS,
        }
    }
    pub(super) fn collections(self) -> &'static [&'static str] {
        match self {
            Self::BindingIndexes => crate::store::bindings::schema::COLLECTIONS,
            Self::Activity | Self::ActivityPage => &["activity"],
            Self::LedgerCollection => &collection_indexes::COLLECTIONS,
            Self::AgentOpening => &opening_index::COLLECTIONS,
            Self::CommandCohort => &["agent_commands"],
            Self::AgentOwnerPage => &["agents"],
            Self::AgentLogSourceKeys => &["agent_log"],
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
            | Self::AgentOwnerPage
            | Self::AgentLogSourceKeys
            | Self::BindingIndexes
            | Self::CommandCohort => "created_indexes",
        }
    }
    pub(super) async fn inspect(
        self,
        database: &mongodb::Database,
    ) -> Result<Vec<(&'static str, Document)>> {
        match self {
            Self::CommandCohort => Ok(command_cohort_index::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("agent_commands", index))
                .collect()),
            Self::BindingIndexes => binding_indexes::inspect(database).await,
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
            Self::AgentLogSourceKeys => Ok(agent_log_source_keys::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("agent_log", index))
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
            Self::CommandCohort if collection == "agent_commands" => {
                command_cohort_index::create(database, index).await
            }
            Self::BindingIndexes => binding_indexes::create(database, collection, index).await,
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
            Self::AgentLogSourceKeys if collection == "agent_log" => {
                agent_log_source_keys::create(database, index).await
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
