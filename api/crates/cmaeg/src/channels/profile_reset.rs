//! Durable owner-authorized partition reset across immutable profile revisions.
use super::*;
use crate::{
    auth::Creator,
    channels::management::{ManagementError, ManagementState},
    providers::{ConversationReset, ProfileTransferRequest, ProviderError},
};

const OPERATION: &str = "cmaeg_profile_reset_v1";

pub(crate) struct ResetInput<'a> {
    pub consumer: &'a ConsumerId,
    pub conversation: &'a ConversationId,
    pub expected_version: u64,
    pub expected_agent_id: &'a str,
    pub operation_id: &'a str,
    pub request_digest: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    owner: String,
    channel_id: String,
    assignment_id: String,
    conversation_id: String,
    operation_id: String,
    source_revision: u64,
    destination_revision: u64,
    source_binding_id: String,
    source_credential_fingerprint: String,
    destination_credential_fingerprint: String,
    next_incarnation: u64,
    request: ProfileTransferRequest,
    outcome: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
enum Outcome {
    Reserved { receipt: ConversationReset },
    Refused { status: u16, code: String },
}

impl Outcome {
    fn result(self) -> Result<ConversationReset, ManagementError> {
        match self {
            Self::Reserved { receipt } => Ok(receipt),
            Self::Refused { status, code } => {
                Err(ManagementError::Provider(ProviderError::Rejected {
                    status,
                    code,
                }))
            }
        }
    }
}

fn lost() -> ManagementError {
    ManagementError::Provider(ProviderError::Unavailable(
        "partition reset custody unavailable; retry the same key".into(),
    ))
}
fn refuse(code: &'static str) -> ManagementError {
    ManagementError::Provider(ProviderError::Rejected {
        status: 409,
        code: code.into(),
    })
}

fn verify_operation(
    operation: &Operation,
    creator: &Creator,
    channel: &ResolvedChannel,
    input: &ResetInput<'_>,
    assignment: &Assignment,
) -> Result<(), ManagementError> {
    if operation.owner != creator.subject
        || operation.channel_id != channel.id.as_str()
        || operation.assignment_id != assignment.id
        || operation.operation_id != input.operation_id
        || operation.conversation_id != input.conversation.as_str()
        || operation.request.request_digest != input.request_digest
        || operation.request.expected_agent_id != input.expected_agent_id
        || operation.request.owner != creator.subject
        || operation.request.organization != creator.organization
    {
        return Err(refuse("partition_reset_key_conflict"));
    }
    Ok(())
}

async fn load_assignment(
    store: &Store,
    channel: &ResolvedChannel,
    input: &ResetInput<'_>,
) -> Result<(Assignment, u64), ManagementError> {
    let (assignment, guard) = prepare(store, channel, input.consumer, input.conversation, false)
        .await
        .map_err(|_| lost())?
        .ok_or_else(|| refuse("partition_reset_mapping_missing"))?;
    let version = guard.and_then(|guard| guard.expected).ok_or_else(lost)?;
    Ok((assignment, version))
}

pub(crate) async fn reset_partition(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &str,
    input: ResetInput<'_>,
) -> Result<ConversationReset, ManagementError> {
    let current = state
        .resolver
        .resolve_current(channel_id)
        .await
        .map_err(|_| lost())?;
    if current.definition.owner.subject != creator.subject
        || (current.definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization)
            != creator.organization
        || !input.consumer.is_default()
    {
        return Err(ManagementError::NotFound);
    }
    let store = state
        .resolver
        .profile_assignment_store()
        .map_err(|_| lost())?;
    let id = index_id(
        OPERATION,
        &[&creator.subject, current.id.as_str(), input.operation_id],
    );
    let existing = store
        .read::<Operation>(&id, false)
        .await
        .map_err(|_| lost())?;
    if let Some((operation, _)) = &existing {
        if operation.conversation_id != input.conversation.as_str()
            || operation.request.request_digest != input.request_digest
            || operation.request.expected_agent_id != input.expected_agent_id
        {
            return Err(refuse("partition_reset_key_conflict"));
        }
    }
    let (mut assignment, assignment_version) = load_assignment(&store, &current, &input).await?;
    let operation = if let Some((operation, _)) = existing {
        verify_operation(&operation, creator, &current, &input, &assignment)?;
        if let Some(outcome) = operation.outcome.clone() {
            return outcome.result();
        }
        operation
    } else {
        if !state.profile_revisions_enabled {
            return Err(refuse("partition_reset_disabled"));
        }
        if current.version != input.expected_version {
            return Err(ManagementError::Conflict {
                expected: input.expected_version,
                actual: Some(current.version),
            });
        }
        if current.definition.mutation.is_some() || assignment.pending_operation.is_some() {
            return Err(lost());
        }
        let source = assignment.view(&current.definition).map_err(|_| lost())?;
        let history = current
            .definition
            .secrets
            .profile_revisions
            .as_ref()
            .ok_or_else(lost)?;
        let destination = current
            .definition
            .profile_revision(history.current_revision)
            .ok_or_else(lost)?;
        let inspected = state
            .providers
            .inspect_conversation(
                &source.provider.slug,
                &source.provider.binding_id,
                input.conversation,
                source.credential,
            )
            .await
            .map_err(ManagementError::Provider)?;
        if inspected.agent_id != input.expected_agent_id {
            return Err(refuse("partition_reset_mapping_changed"));
        }
        if inspected.agent_status.as_deref() != Some("deleted") {
            return Err(refuse("partition_reset_agent_not_deleted"));
        }
        let destination_generation = if source.revision == destination.revision {
            inspected.generation
        } else {
            state
                .providers
                .conversation_catalog(
                    &destination.provider.slug,
                    &destination.provider.binding_id,
                    destination.credential,
                )
                .await
                .map_err(ManagementError::Provider)?
                .generation
        };
        let operation = Operation {
            owner: creator.subject.clone(),
            channel_id: current.id.to_string(),
            assignment_id: assignment.id.clone(),
            conversation_id: input.conversation.to_string(),
            operation_id: input.operation_id.into(),
            source_revision: source.revision,
            destination_revision: destination.revision,
            source_binding_id: source.provider.binding_id.to_string(),
            source_credential_fingerprint: hash_secret(source.credential.as_str()),
            destination_credential_fingerprint: hash_secret(destination.credential.as_str()),
            next_incarnation: assignment
                .incarnation
                .max(inspected.incarnation)
                .checked_add(1)
                .ok_or_else(lost)?,
            request: ProfileTransferRequest {
                destination_binding_id: destination.provider.binding_id.to_string(),
                expected_agent_id: inspected.agent_id,
                expected_generation: inspected.generation,
                expected_destination_generation: destination_generation,
                expected_incarnation: inspected.incarnation,
                owner: creator.subject.clone(),
                organization: creator.organization,
                request_digest: input.request_digest.into(),
            },
            outcome: None,
        };
        assignment.pending_operation = Some(id.clone());
        let channel_guard = Some((current.definition.record_id.to_string(), current.version));
        let writes = [
            store
                .prepare(
                    &assignment.id,
                    &assignment.owner,
                    &assignment,
                    Write {
                        expected: Some(assignment_version),
                        channel: channel_guard.clone(),
                        ..Default::default()
                    },
                )
                .map_err(|_| lost())?,
            store
                .prepare(
                    &id,
                    &operation.owner,
                    &operation,
                    Write {
                        channel: channel_guard,
                        ..Default::default()
                    },
                )
                .map_err(|_| lost())?,
        ];
        if store.batch(&writes).await.is_err() {
            // An acknowledged peer or lost response must select the durable
            // operation, never the newly computed candidate.
            let (retained, _) = store
                .read::<Operation>(&id, false)
                .await
                .map_err(|_| lost())?
                .ok_or_else(lost)?;
            verify_operation(&retained, creator, &current, &input, &assignment)?;
            if let Some(outcome) = retained.outcome.clone() {
                return outcome.result();
            }
            retained
        } else {
            operation
        }
    };
    let (retained, _) = load_assignment(&store, &current, &input).await?;
    if retained.pending_operation.as_deref() != Some(id.as_str())
        || retained.revision != operation.source_revision
    {
        return Err(lost());
    }
    let outcome = match invoke(state, &current, &operation, input.conversation).await {
        Ok(receipt) => Outcome::Reserved { receipt },
        Err(ProviderError::Rejected { status: 409, code })
            if matches!(
                code.as_str(),
                "partition_reset_disabled"
                    | "partition_reset_key_conflict"
                    | "partition_reset_existing_agent"
                    | "partition_reset_affinity_unsupported"
                    | "partition_reset_binding_not_ready"
                    | "partition_reset_mapping_retired"
                    | "partition_reset_agent_missing"
                    | "partition_reset_agent_not_deleted"
                    | "partition_reset_cleanup_pending"
                    | "partition_reset_parents_unavailable"
                    | "partition_reset_mapping_missing"
                    | "partition_reset_mapping_changed"
                    | "partition_reset_destination_exists"
                    | "partition_reset_capacity"
                    | "partition_reset_unsupported"
            ) =>
        {
            Outcome::Refused { status: 409, code }
        }
        Err(error) => return Err(ManagementError::Provider(error)),
    };
    finish(state, &store, &id, &operation, &input, outcome).await
}

async fn invoke(
    state: &ManagementState,
    channel: &ResolvedChannel,
    operation: &Operation,
    conversation: &ConversationId,
) -> Result<ConversationReset, ProviderError> {
    let unavailable =
        || ProviderError::Unavailable("retained profile reset authority unavailable".into());
    let source = channel
        .definition
        .profile_revision(operation.source_revision)
        .ok_or_else(unavailable)?;
    let destination = channel
        .definition
        .profile_revision(operation.destination_revision)
        .ok_or_else(unavailable)?;
    if source.provider.binding_id.as_str() != operation.source_binding_id
        || destination.provider.binding_id.as_str() != operation.request.destination_binding_id
        || hash_secret(source.credential.as_str()) != operation.source_credential_fingerprint
        || hash_secret(destination.credential.as_str())
            != operation.destination_credential_fingerprint
    {
        return Err(unavailable());
    }
    if source.revision == destination.revision {
        let mut receipt = state
            .providers
            .reset_conversation(
                &source.provider.slug,
                &source.provider.binding_id,
                conversation,
                &operation.operation_id,
                serde_json::json!({
                    "expected_agent_id":operation.request.expected_agent_id,
                    "owner":operation.owner,
                    "organization":operation.request.organization,
                    "request_digest":operation.request.request_digest,
                    "idempotency_binding_id":source.provider.binding_id,
                }),
            )
            .await?;
        if receipt.generation != operation.request.expected_generation
            || receipt.incarnation
                != operation
                    .request
                    .expected_incarnation
                    .checked_add(1)
                    .ok_or_else(unavailable)?
        {
            return Err(unavailable());
        }
        receipt.incarnation = operation.next_incarnation;
        return Ok(receipt);
    }
    let receipt = state
        .providers
        .transfer_profile(
            &source.provider.slug,
            &source.provider.binding_id,
            conversation,
            &operation.operation_id,
            &operation.request,
        )
        .await?;
    Ok(ConversationReset {
        binding_id: receipt.destination_binding_id,
        generation: receipt.destination_generation,
        conversation_id: receipt.conversation_id,
        incarnation: operation.next_incarnation,
        previous_agent_id: receipt.source_agent_id,
        previous_conversation_id: receipt.source_conversation_id,
        successor_agent_id: receipt.destination_agent_id,
        state: receipt.state,
    })
}

async fn finish(
    state: &ManagementState,
    store: &Store,
    id: &str,
    admitted: &Operation,
    input: &ResetInput<'_>,
    outcome: Outcome,
) -> Result<ConversationReset, ManagementError> {
    for _ in 0..4 {
        let current = state
            .resolver
            .resolve_current(&admitted.channel_id)
            .await
            .map_err(|_| lost())?;
        let (mut operation, operation_version) = store
            .read::<Operation>(id, false)
            .await
            .map_err(|_| lost())?
            .ok_or_else(lost)?;
        if let Some(retained) = operation.outcome.clone() {
            return if retained == outcome {
                retained.result()
            } else {
                Err(lost())
            };
        }
        if operation != *admitted {
            return Err(lost());
        }
        let (mut assignment, assignment_version) = load_assignment(store, &current, input).await?;
        if assignment.id != admitted.assignment_id
            || assignment.pending_operation.as_deref() != Some(id)
            || assignment.revision != admitted.source_revision
            || assignment.owner != admitted.owner
        {
            return Err(lost());
        }
        if matches!(outcome, Outcome::Reserved { .. }) {
            let target = current
                .definition
                .profile_revision(admitted.destination_revision)
                .ok_or_else(lost)?;
            if target.provider.binding_id.as_str() != admitted.request.destination_binding_id
                || hash_secret(target.credential.as_str())
                    != admitted.destination_credential_fingerprint
            {
                return Err(lost());
            }
            assignment.first_revision =
                Some(assignment.first_revision.unwrap_or(assignment.revision));
            assignment.revision = target.revision;
            assignment.binding_id = target.provider.binding_id.to_string();
            assignment.profile_digest = target.profile_digest.into();
            assignment.credential_fingerprint = admitted.destination_credential_fingerprint.clone();
            assignment.incarnation = admitted.next_incarnation;
        }
        assignment.pending_operation = None;
        operation.outcome = Some(outcome.clone());
        let guard = Some((current.definition.record_id.to_string(), current.version));
        let writes = [
            store
                .prepare(
                    &assignment.id,
                    &assignment.owner,
                    &assignment,
                    Write {
                        expected: Some(assignment_version),
                        channel: guard.clone(),
                        ..Default::default()
                    },
                )
                .map_err(|_| lost())?,
            store
                .prepare(
                    id,
                    &operation.owner,
                    &operation,
                    Write {
                        expected: Some(operation_version),
                        channel: guard,
                        ..Default::default()
                    },
                )
                .map_err(|_| lost())?,
        ];
        if store.batch(&writes).await.is_ok() {
            return outcome.result();
        }
    }
    Err(lost())
}
