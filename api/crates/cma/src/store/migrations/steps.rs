//! The explicit manifest registry is also the migrator's reader floor.
use super::{
    account_erasure_work, activity_audit_index, activity_indexes, activity_page_index,
    agent_log_active_proofs, agent_log_segments, agent_log_source_keys, agent_owner_index,
    binding_indexes, collection_indexes, command_cohort_index, measured_batches, measured_custody,
    measured_index, opening_index, sandbox_usage_capture, turn_policy_index, usage_projections,
    Result,
};
use mongodb::bson::{doc, Document};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    AccountErasureWork,
    Activity,
    ActivityPage,
    ActivityAudit,
    LedgerCollection,
    AgentOpening,
    AgentOwnerPage,
    TurnPolicyDue,
    AgentLogSegments,
    AgentLogSourceKeys,
    AgentLogActiveProofs,
    BindingIndexes,
    CommandCohort,
    SandboxUsageCapture,
    MeasuredCustody,
    UsageProjections,
    MeasuredBatches,
    MeasuredIndex,
}

impl Step {
    pub(super) const ALL: [Self; 18] = [
        Self::AccountErasureWork,
        Self::AgentLogActiveProofs,
        Self::MeasuredIndex,
        Self::MeasuredBatches,
        Self::MeasuredCustody,
        Self::SandboxUsageCapture,
        Self::Activity,
        Self::ActivityPage,
        Self::ActivityAudit,
        Self::LedgerCollection,
        Self::AgentOpening,
        Self::AgentOwnerPage,
        Self::TurnPolicyDue,
        Self::AgentLogSegments,
        Self::AgentLogSourceKeys,
        Self::BindingIndexes,
        Self::CommandCohort,
        Self::UsageProjections,
    ];
    pub(super) fn parse(id: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|step| step.id() == id)
            .ok_or("unknown migration step")
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::AccountErasureWork => account_erasure_work::ID,
            Self::CommandCohort => command_cohort_index::ID,
            Self::MeasuredCustody => measured_custody::ID,
            Self::UsageProjections => usage_projections::ID,
            Self::MeasuredBatches => measured_batches::ID,
            Self::MeasuredIndex => measured_index::ID,
            Self::SandboxUsageCapture => sandbox_usage_capture::ID,
            Self::BindingIndexes => binding_indexes::ID,
            Self::Activity => activity_indexes::ID,
            Self::ActivityPage => activity_page_index::ID,
            Self::ActivityAudit => activity_audit_index::ID,
            Self::LedgerCollection => collection_indexes::ID,
            Self::AgentOpening => opening_index::ID,
            Self::AgentOwnerPage => agent_owner_index::ID,
            Self::TurnPolicyDue => turn_policy_index::ID,
            Self::AgentLogSegments => agent_log_segments::ID,
            Self::AgentLogSourceKeys => agent_log_source_keys::ID,
            Self::AgentLogActiveProofs => agent_log_active_proofs::ID,
        }
    }
    pub(super) fn checksum(self) -> String {
        match self {
            Self::AccountErasureWork => account_erasure_work::checksum(),
            Self::CommandCohort => command_cohort_index::checksum(),
            Self::MeasuredCustody => measured_custody::checksum(),
            Self::UsageProjections => usage_projections::checksum(),
            Self::MeasuredBatches => measured_batches::checksum(),
            Self::MeasuredIndex => measured_index::checksum(),
            Self::SandboxUsageCapture => sandbox_usage_capture::checksum(),
            Self::BindingIndexes => binding_indexes::checksum(),
            Self::Activity => activity_indexes::checksum(),
            Self::ActivityPage => activity_page_index::checksum(),
            Self::ActivityAudit => activity_audit_index::checksum(),
            Self::LedgerCollection => collection_indexes::checksum(),
            Self::AgentOpening => opening_index::checksum(),
            Self::AgentOwnerPage => agent_owner_index::checksum(),
            Self::TurnPolicyDue => turn_policy_index::checksum(),
            Self::AgentLogSegments => agent_log_segments::checksum(),
            Self::AgentLogSourceKeys => agent_log_source_keys::checksum(),
            Self::AgentLogActiveProofs => agent_log_active_proofs::checksum(),
        }
    }
    pub(super) fn manifest(self) -> String {
        format!("manifest:{}", self.id())
    }
    pub(super) fn max_changes(self) -> i64 {
        match self {
            Self::AccountErasureWork => 4,
            Self::CommandCohort => 1,
            Self::MeasuredCustody | Self::MeasuredBatches | Self::AgentLogActiveProofs => 1,
            Self::MeasuredIndex => 5,
            Self::UsageProjections => usage_projections::INDEX_COUNT,
            Self::SandboxUsageCapture => crate::store::sandbox_usage_export::schema::MAX_STEPS,
            Self::BindingIndexes => 11,
            Self::Activity => 2,
            Self::ActivityPage | Self::ActivityAudit => 1,
            Self::LedgerCollection => 5,
            Self::AgentOpening
            | Self::AgentOwnerPage
            | Self::AgentLogSourceKeys
            | Self::TurnPolicyDue => 1,
            Self::AgentLogSegments => agent_log_segments::MAX_SCHEMA_STEPS,
        }
    }
    pub(super) fn collections(self) -> &'static [&'static str] {
        match self {
            Self::AccountErasureWork => crate::store::account_deletions::worker_schema::COLLECTIONS,
            Self::MeasuredCustody => &[measured_custody::COLLECTION],
            Self::UsageProjections => &[usage_projections::COLLECTION],
            Self::MeasuredBatches => &[measured_batches::COLLECTION],
            Self::MeasuredIndex => &[
                measured_index::PROJECTIONS,
                measured_index::MANIFESTS,
                measured_index::BATCHES,
            ],
            Self::AgentLogActiveProofs => &[agent_log_active_proofs::COLLECTION],
            Self::SandboxUsageCapture => crate::store::sandbox_usage_export::COLLECTIONS,
            Self::BindingIndexes => crate::store::bindings::schema::COLLECTIONS,
            Self::Activity | Self::ActivityPage | Self::ActivityAudit => &["activity"],
            Self::LedgerCollection => &collection_indexes::COLLECTIONS,
            Self::AgentOpening => &opening_index::COLLECTIONS,
            Self::CommandCohort => &["agent_commands"],
            Self::AgentOwnerPage | Self::TurnPolicyDue => &["agents"],
            Self::AgentLogSourceKeys => &["agent_log"],
            Self::AgentLogSegments => crate::store::agent_log_segments::COLLECTIONS,
        }
    }
    pub(super) fn effect_field(self) -> &'static str {
        match self {
            Self::AgentLogSegments | Self::SandboxUsageCapture | Self::AccountErasureWork => {
                "applied_schema_steps"
            }
            Self::Activity
            | Self::ActivityPage
            | Self::ActivityAudit
            | Self::LedgerCollection
            | Self::AgentOpening
            | Self::AgentOwnerPage
            | Self::TurnPolicyDue
            | Self::AgentLogSourceKeys
            | Self::CommandCohort
            | Self::MeasuredCustody
            | Self::UsageProjections
            | Self::MeasuredBatches
            | Self::MeasuredIndex
            | Self::AgentLogActiveProofs
            | Self::BindingIndexes => "created_indexes",
        }
    }
    pub(super) async fn inspect(
        self,
        database: &mongodb::Database,
    ) -> Result<Vec<(&'static str, Document)>> {
        match self {
            Self::AccountErasureWork => account_erasure_work::inspect(database).await,
            Self::CommandCohort => Ok(command_cohort_index::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("agent_commands", index))
                .collect()),
            Self::MeasuredCustody => measured_custody::inspect(database).await,
            Self::UsageProjections => usage_projections::inspect(database).await,
            Self::MeasuredBatches => measured_batches::inspect(database).await,
            Self::MeasuredIndex => measured_index::inspect(database).await,
            Self::AgentLogActiveProofs => agent_log_active_proofs::inspect(database).await,
            Self::SandboxUsageCapture => Err("usage migration requires schema-unit inspection"),
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
            Self::ActivityAudit => Ok(activity_audit_index::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("activity", index))
                .collect()),
            Self::LedgerCollection => collection_indexes::inspect(database).await,
            Self::AgentOpening => opening_index::inspect(database).await,
            Self::TurnPolicyDue => Ok(turn_policy_index::inspect(database)
                .await?
                .into_iter()
                .map(|index| ("agents", index))
                .collect()),
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
            Self::AccountErasureWork => {
                account_erasure_work::create(database, collection, index).await
            }
            Self::CommandCohort if collection == "agent_commands" => {
                command_cohort_index::create(database, index).await
            }
            Self::MeasuredIndex => measured_index::create(database, collection, index).await,
            Self::MeasuredBatches if collection == measured_batches::COLLECTION => {
                measured_batches::create(database, index).await
            }
            Self::AgentLogActiveProofs if collection == agent_log_active_proofs::COLLECTION => {
                agent_log_active_proofs::create(database, index).await
            }
            Self::MeasuredCustody if collection == measured_custody::COLLECTION => {
                measured_custody::create(database, index).await
            }
            Self::UsageProjections if collection == usage_projections::COLLECTION => {
                usage_projections::create(database, index).await
            }
            Self::BindingIndexes => binding_indexes::create(database, collection, index).await,
            Self::Activity if collection == "activity" => {
                activity_indexes::create(database, index).await
            }
            Self::ActivityPage if collection == "activity" => {
                activity_page_index::create(database, index).await
            }
            Self::ActivityAudit if collection == "activity" => {
                activity_audit_index::create(database, index).await
            }
            Self::LedgerCollection => collection_indexes::create(database, collection, index).await,
            Self::AgentOpening => opening_index::create(database, collection, index).await,
            Self::TurnPolicyDue if collection == "agents" => {
                turn_policy_index::create(database, index).await
            }
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
        doc! {"kind":"index_abort","_id":"destructive_ddl","migration":step.id(),"checksum":step.checksum()},
    ]).chain([doc! {"kind":"lease","_id":"runner"}]).collect();
    doc! {"$or":[
        {"schema":{"$ne":1_i64}}, {"schema":{"$not":{"$type":["int","long"]}}},
        {"schema":{"$type":"array"}}, {"kind":{"$not":{"$type":"string"}}},
        {"kind":{"$type":"array"}}, {"$nor":allowed}
    ]}
}
