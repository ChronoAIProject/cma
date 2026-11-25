//! Owner-authorized inspection of retained records without creating an address.
use super::{
    channel_endpoints::channel_endpoints,
    management::EndpointResponse,
    management_responses::management_error,
    problems::{no_store, problem},
    record_ids::canonical_record_id,
};
use crate::{
    auth::Creator,
    channels::management::{self, ManagementError, ManagementState, RecordedChannel},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{
    channel::{ChannelMutationKind, ConformanceLevel},
    ids::{ChannelId, RecordId},
    problem::ProblemCode,
    profile::NyxIdIdentity,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
struct ChannelRecordManagement {
    record_id: RecordId,
    /// Storage version of this read; does not authorize a mutation or retry.
    version: u64,
    record: ManagedRecord,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ManagedRecord {
    Valid {
        name: String,
        owner: NyxIdIdentity,
        provider: ManagedProvider,
        creation: RecordedCreation,
        address: Box<RecordedAddress>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pending_operation: Option<RecordedOperation>,
        /// Expected version for the retained Delete intent of a retired incomplete channel.
        #[serde(skip_serializing_if = "Option::is_none")]
        retirement_expected_version: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retirement: Option<LegacyRetirement>,
    },
    Invalid,
}

#[derive(Debug, Serialize, ToSchema)]
struct LegacyRetirement {
    expected_version: u64,
    state: LegacyRetirementState,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum LegacyRetirementState {
    Available,
    BlockedMissingCredential,
    Pending,
    CleanupUnconfirmed,
}

#[derive(Debug, Serialize, ToSchema)]
struct ManagedProvider {
    slug: String,
    level: ConformanceLevel,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum RecordedCreation {
    Legacy,
    Incomplete,
    Complete,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RecordedAddress {
    Available {
        channel_id: ChannelId,
        endpoints: EndpointResponse,
    },
    NotRecorded,
    Unverifiable,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum RecordedOperation {
    Create,
    Update,
    ProfileRevision,
    Delete,
    AuditRouting,
    AuditAttachments,
}

impl From<ChannelMutationKind> for RecordedOperation {
    fn from(operation: ChannelMutationKind) -> Self {
        match operation {
            ChannelMutationKind::Create => Self::Create,
            ChannelMutationKind::Update => Self::Update,
            ChannelMutationKind::ProfileRevision => Self::ProfileRevision,
            ChannelMutationKind::Delete => Self::Delete,
            ChannelMutationKind::AuditRouting => Self::AuditRouting,
            ChannelMutationKind::AuditAttachments => Self::AuditAttachments,
        }
    }
}

fn project(
    state: &ManagementState,
    record_id: RecordId,
    current: RecordedChannel,
) -> ChannelRecordManagement {
    let record = if current.valid {
        let definition = &current.definition;
        let (creation, address) = match &definition.creation {
            None => (RecordedCreation::Legacy, RecordedAddress::NotRecorded),
            Some(creation) => {
                let address = if crate::console::channel_ids::opens_to(
                    &creation.channel_id,
                    &record_id,
                    &state.master_keys,
                    &state.records,
                )
                .is_ok()
                {
                    RecordedAddress::Available {
                        channel_id: creation.channel_id.clone(),
                        endpoints: channel_endpoints(state, &creation.channel_id, definition),
                    }
                } else {
                    RecordedAddress::Unverifiable
                };
                (
                    if creation.ready {
                        RecordedCreation::Complete
                    } else {
                        RecordedCreation::Incomplete
                    },
                    address,
                )
            }
        };
        ManagedRecord::Valid {
            name: definition.name.clone(),
            owner: definition.owner.clone(),
            provider: ManagedProvider {
                slug: definition.provider.slug.to_string(),
                level: definition.provider.level,
            },
            creation,
            address: Box::new(address),
            retirement: if definition.creation.is_none() {
                match definition.mutation.as_ref() {
                    Some(mutation) if mutation.operation == ChannelMutationKind::Delete => {
                        Some(LegacyRetirement {
                            expected_version: mutation.original_version,
                            state: if mutation.effect.is_some() {
                                LegacyRetirementState::CleanupUnconfirmed
                            } else if definition.secrets.provider_credential.is_none() {
                                LegacyRetirementState::BlockedMissingCredential
                            } else {
                                LegacyRetirementState::Pending
                            },
                        })
                    }
                    None if state.records.channel_mutation_custody() => Some(LegacyRetirement {
                        expected_version: current.version,
                        state: if definition.secrets.provider_credential.is_some() {
                            LegacyRetirementState::Available
                        } else {
                            LegacyRetirementState::BlockedMissingCredential
                        },
                    }),
                    _ => None,
                }
            } else {
                None
            },
            pending_operation: if current.retirement_expected_version.is_some() {
                Some(RecordedOperation::Delete)
            } else {
                definition
                    .mutation
                    .as_ref()
                    .map(|mutation| mutation.operation.into())
            },
            retirement_expected_version: current.retirement_expected_version,
        }
    } else {
        ManagedRecord::Invalid
    };
    ChannelRecordManagement {
        record_id,
        version: current.version,
        record,
    }
}

#[utoipa::path(get, path="/v1/channel-records/{record_id}/management",
    operation_id="getChannelRecordManagement", tag="channels",
    security(("creatorBearer"=[])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("record_id"=String, Path)),
    responses((status=200, body=ChannelRecordManagement), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn read(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(id): Path<String>,
) -> Response {
    let id = match canonical_record_id(id) {
        Ok(id) => id,
        Err(error) => return management_error(ManagementError::Invalid(error)),
    };
    match management::load_by_record(&state, &creator, &id).await {
        Ok(current) => no_store(Json(project(&state, id, current)).into_response()),
        Err(error) => management_error(error),
    }
}

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct LegacyDeleteQuery {
    #[param(required = true, nullable = false)]
    expected_version: Option<u64>,
}

#[utoipa::path(delete, path="/v1/channel-records/{record_id}/management",
    operation_id="retireLegacyChannelRecord", tag="channels",
    security(("creatorBearer"=[])),
    params(("X-CMAEG-Owner" = Option<String>, Header, description = "Organization owner UUID; caller must be an authorized NyxID organization administrator"), ("record_id"=String, Path), LegacyDeleteQuery),
    responses((status=204), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn retire(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(id): Path<String>,
    query: Result<Query<LegacyDeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let id = match canonical_record_id(id) {
        Ok(id) => id,
        Err(error) => return management_error(ManagementError::Invalid(error)),
    };
    let Some(expected_version) = query.ok().and_then(|Query(query)| query.expected_version) else {
        return no_store(problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "expected_version is required",
        ));
    };
    match management::delete_legacy(&state, &creator, &id, expected_version).await {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => management_error(error),
    }
}
