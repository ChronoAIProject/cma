//! Owner-scoped profile revision publication over immutable provider bindings.
use super::{
    definition::hash_secret,
    management::{binding_request, load, open_pointer, seal_definition},
    mutation::{Intent, Reservation},
    profile_assignment,
    state::{ManagementError, ManagementState},
};
use crate::{auth::Creator, connections::store::index_id, ingress::routing_store::PlanStore};
use cmaeg_core::{
    channel::{
        ChannelDefinition, ChannelMutationKind, ProfilePublication, ProfileRevisions,
        RetainedProfileRevision, Secret,
    },
    ids::{BindingId, ChannelId, ConversationId},
    profile::EventWorkerProfile,
    ValidationError,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) fn configure(config: &crate::Config, state: &mut ManagementState) -> Result<(), String> {
    if config.profile_revisions_enabled
        && (!config.routing.recovery_enabled
            || !config.routing.admission_enabled
            || !state.records.channel_mutation_custody()
            || state.records.database().is_none())
    {
        return Err(
            "channel profile revisions require durable routed admission and local mutation custody"
                .into(),
        );
    }
    state.profile_revisions_enabled = config.profile_revisions_enabled;
    state
        .resolver
        .require_current_profile_reads(config.profile_revisions_enabled);
    Ok(())
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionRequest {
    pub expected_version: u64,
    pub profile: EventWorkerProfile,
    pub agent_key: Secret,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RevisionReceipt {
    pub channel_id: String,
    pub version: u64,
    pub profile_revision: u64,
    pub profile_digest: String,
    pub baseline_complete: bool,
}

fn refuse(detail: &'static str) -> ManagementError {
    ManagementError::Invalid(ValidationError::new("/profile", detail))
}
fn unavailable() -> ManagementError {
    ManagementError::Provider(crate::providers::ProviderError::Unavailable(
        "profile revision custody unavailable".into(),
    ))
}
pub(crate) fn can_baseline(definition: &ChannelDefinition) -> bool {
    definition.consumer(None).is_some_and(|consumer| {
        consumer.provider.slug.as_str() == "cma_codex"
            && consumer.provider.target.is_profile()
            && consumer
                .profile
                .environment
                .as_ref()
                .and_then(|environment| environment.get("session_affinity"))
                .and_then(serde_json::Value::as_str)
                != Some("source_user")
    })
}

fn check(state: &ManagementState, definition: &ChannelDefinition) -> Result<(), ManagementError> {
    if !state.profile_revisions_enabled {
        return Err(refuse("profile revision writes are disabled"));
    }
    if !can_baseline(definition) {
        return Err(refuse(
            "profile revisions require a CMA conversation-affinity channel",
        ));
    }
    super::creation::require_ready(definition)
}
fn receipt(
    id: &ChannelId,
    definition: &ChannelDefinition,
    version: u64,
) -> Result<RevisionReceipt, ManagementError> {
    let history = definition
        .secrets
        .profile_revisions
        .as_ref()
        .ok_or_else(unavailable)?;
    Ok(RevisionReceipt {
        channel_id: id.to_string(),
        version,
        profile_revision: history.current_revision,
        profile_digest: definition.profile_digest.clone(),
        baseline_complete: history.baseline_complete,
    })
}

async fn save(
    state: &ManagementState,
    channel: &ChannelId,
    definition: &ChannelDefinition,
    version: u64,
) -> Result<u64, ManagementError> {
    let blob = seal_definition(&state.master_keys, definition)?;
    let next = match state
        .records
        .put(
            &definition.record_id,
            &definition.owner.subject,
            &blob,
            Some(version),
        )
        .await
    {
        Ok(version) => version,
        Err(error) => {
            let row = state
                .records
                .get(
                    state
                        .records
                        .record_url(definition.record_id.as_str())?
                        .as_str(),
                )
                .await?;
            if Some(row.version) != version.checked_add(1) || row.blob != blob {
                return Err(error.into());
            }
            row.version
        }
    };
    state
        .index
        .changed(&definition.record_id, &definition.owner.subject, next);
    state.resolver.invalidate(channel.as_str());
    Ok(next)
}

/// The reader floor and legacy writer drain are required before enabling the
/// writer. Existing admitted work and mappings are pinned before cutover.
pub async fn baseline(
    state: &ManagementState,
    creator: &Creator,
    id: &ChannelId,
    expected: u64,
) -> Result<RevisionReceipt, ManagementError> {
    let pointer = open_pointer(state, id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let loaded = load(state, creator, id).await?;
    check(state, &loaded.definition)?;
    super::mutation::available(&loaded.definition, loaded.record.version)?;
    if loaded.record.version != expected {
        return Err(ManagementError::Conflict {
            expected,
            actual: Some(loaded.record.version),
        });
    }
    if loaded
        .definition
        .secrets
        .profile_revisions
        .as_ref()
        .is_some_and(|history| history.baseline_complete)
    {
        return receipt(id, &loaded.definition, loaded.record.version);
    }
    if loaded.definition.secrets.profile_revisions.is_none() {
        let mut definition = loaded.definition;
        definition.secrets.profile_revisions = Some(ProfileRevisions {
            current_revision: 1,
            baseline_complete: false,
            last_publication: None,
            retained: Vec::new(),
        });
        save(state, id, &definition, loaded.record.version).await?;
    }
    let current = state
        .resolver
        .resolve_current(id.as_str())
        .await
        .map_err(|_| unavailable())?;
    let store = state
        .resolver
        .profile_assignment_store()
        .map_err(|_| unavailable())?;
    let credential = current
        .definition
        .secrets
        .provider_credential
        .as_ref()
        .ok_or_else(unavailable)?;
    let provider = &current.definition.provider;
    let catalog = state
        .providers
        .conversation_catalog(&provider.slug, &provider.binding_id, credential)
        .await
        .map_err(ManagementError::Provider)?;
    let mut conversations: BTreeSet<String> =
        catalog.transport_conversations.iter().cloned().collect();
    let plans = PlanStore::new((*state.records).clone(), state.master_keys.clone());
    let mut after = None;
    for page_number in 0..128 {
        let page = plans
            .recover_page(&current.definition.owner.subject, after.as_deref(), 32)
            .await
            .map_err(|_| unavailable())?;
        if !page.issues.is_empty() {
            return Err(unavailable());
        }
        for plan in page.plans.into_iter().filter(|plan| {
            plan.identity.record_id == current.definition.record_id
                && plan.identity.channel_id == *id
        }) {
            for target in plan
                .targets
                .into_iter()
                .filter(|target| target.launch.consumer.is_default())
            {
                if target.launch.provider != *provider {
                    return Err(unavailable());
                }
                conversations.insert(target.launch.event.conversation_id.to_string());
            }
        }
        after = page.next_cursor;
        if after.is_none() {
            break;
        }
        if page_number == 127 {
            return Err(unavailable());
        }
    }
    for conversation in conversations {
        profile_assignment::baseline(
            &store,
            &current,
            &ConversationId::new(conversation).map_err(ManagementError::Invalid)?,
        )
        .await
        .map_err(|_| unavailable())?;
    }
    let confirmed = state
        .providers
        .conversation_catalog(&provider.slug, &provider.binding_id, credential)
        .await
        .map_err(ManagementError::Provider)?;
    if confirmed != catalog {
        return Err(unavailable());
    }
    let mut definition = current.definition.clone();
    definition
        .secrets
        .profile_revisions
        .as_mut()
        .ok_or_else(unavailable)?
        .baseline_complete = true;
    let version = save(state, id, &definition, current.version).await?;
    receipt(id, &definition, version)
}

pub async fn publish(
    state: &ManagementState,
    creator: &Creator,
    id: &ChannelId,
    request: RevisionRequest,
) -> Result<RevisionReceipt, ManagementError> {
    let pointer = open_pointer(state, id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, id).await?;
    check(state, &current.definition)?;
    request
        .profile
        .validate()
        .map_err(ManagementError::Invalid)?;
    if request.profile.metadata.owner != current.definition.profile.metadata.owner
        || request.profile.metadata.provider != current.definition.provider.slug
        || request
            .profile
            .environment
            .as_ref()
            .and_then(|environment| environment.get("session_affinity"))
            .and_then(serde_json::Value::as_str)
            == Some("source_user")
    {
        return Err(refuse(
            "profile owner, provider and conversation affinity must be retained",
        ));
    }
    let fingerprint = hash_secret(&cmaeg_core::profile::canonical_json(
        serde_json::json!({"expected_version":request.expected_version, "profile":request.profile, "agent_key":hash_secret(request.agent_key.as_str())}),
    ));
    let history = current
        .definition
        .secrets
        .profile_revisions
        .as_ref()
        .filter(|history| history.baseline_complete)
        .ok_or_else(|| {
            refuse("partition baseline must be completed before revision publication")
        })?;
    if history.last_publication.as_ref().is_some_and(|published| {
        published.expected_version == request.expected_version
            && published.request_fingerprint == fingerprint
    }) {
        return receipt(id, &current.definition, current.record.version);
    }
    if super::mutation::original_version(&current.definition, current.record.version)
        != request.expected_version
    {
        return Err(ManagementError::Conflict {
            expected: request.expected_version,
            actual: Some(current.record.version),
        });
    }
    // The existing channel mutation journal can retain at most sixteen steps,
    // including each binding retirement and an optional connection disconnect.
    if history.retained.len() >= 12 {
        return Err(refuse(
            "profile revision history exceeds channel retirement capacity",
        ));
    }
    let old = current
        .definition
        .secrets
        .provider_credential
        .as_ref()
        .ok_or_else(unavailable)?;
    if request.agent_key.as_str().is_empty()
        || &request.agent_key == old
        || history
            .retained
            .iter()
            .any(|revision| revision.credential == request.agent_key)
    {
        return Err(refuse(
            "a new provider binding requires its own scoped Agent credential",
        ));
    }
    let mut desired = current.definition.clone();
    let history = desired
        .secrets
        .profile_revisions
        .as_mut()
        .ok_or_else(unavailable)?;
    history.retained.push(RetainedProfileRevision {
        revision: history.current_revision,
        profile_digest: desired.profile_digest.clone(),
        profile: serde_json::to_value(&desired.profile).map_err(|_| unavailable())?,
        provider: desired.provider.clone(),
        credential: old.clone(),
    });
    history.current_revision = history
        .current_revision
        .checked_add(1)
        .ok_or_else(unavailable)?;
    history.last_publication = Some(ProfilePublication {
        expected_version: request.expected_version,
        request_fingerprint: fingerprint.clone(),
    });
    desired.profile = request.profile;
    desired.profile_digest = desired.profile.digest();
    desired.provider.binding_id = BindingId::new(index_id(
        "cmaeg-profile-revision-v1",
        &[desired.record_id.as_str(), &fingerprint],
    ))
    .map_err(ManagementError::Invalid)?;
    desired.provider.capabilities.clear();
    desired.secrets.agent_key_hash = hash_secret(request.agent_key.as_str());
    desired.secrets.provider_credential = Some(request.agent_key);
    desired.mutation = None;
    desired.validate().map_err(ManagementError::Invalid)?;
    let mut reservation = Reservation::acquire(
        &state.records,
        &state.master_keys,
        &current.definition,
        current.record.version,
        Intent {
            operation: ChannelMutationKind::ProfileRevision,
            expected_version: request.expected_version,
            fingerprint,
            desired: &desired,
        },
    )
    .await?;
    desired = reservation.desired.clone();
    let step = format!(
        "bind:{}:{}",
        desired.provider.slug, desired.provider.binding_id
    );
    if !reservation.done(&step) {
        let provider_request = binding_request(state, &desired)?;
        reservation
            .effect(&state.records, &state.master_keys, step)
            .await?;
        let binding = match state
            .providers
            .bind_custodied(
                &desired.provider.slug,
                &desired.provider.binding_id,
                &provider_request,
            )
            .await
        {
            Ok(binding) => binding,
            Err(error) => {
                if !error.unconfirmed {
                    reservation
                        .retryable(&state.records, &state.master_keys)
                        .await?;
                }
                return Err(ManagementError::Provider(error.error));
            }
        };
        if binding.binding_id != desired.provider.binding_id {
            return Err(unavailable());
        }
        desired.provider.level = binding.level;
        desired.provider.capabilities = binding.capabilities;
        reservation
            .confirmed(&state.records, &state.master_keys, desired.clone())
            .await?;
    }
    let (_, version) = reservation
        .publish(&state.records, &state.master_keys, &desired)
        .await?;
    state
        .index
        .changed(&desired.record_id, &creator.subject, version);
    state.resolver.invalidate(id.as_str());
    receipt(id, &desired, version)
}
