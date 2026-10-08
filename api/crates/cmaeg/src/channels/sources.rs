//! Owner-managed source attachment and detachment under the existing CAS guard.
pub use super::source_secrets::SourceCredentials;
use super::{
    definition_sources::{build_sources, validate_source_requirements},
    management::{custom_attachments_allowed, load},
    mutation_metrics::{observe, Operation},
    routing_policy::{check_version, persist},
    source_secrets,
    state::{LoadedChannel, ManagementError, ManagementState},
    target::{open_target, ChannelTarget},
    write_gate, SourceRequest,
};
use crate::auth::Creator;
use cmaeg_core::{
    channel::{ChannelDefinition, DefaultReply, PartitionKey, Source, SourceId},
    ValidationError,
};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceAttachRequest {
    pub expected_version: u64,
    pub source: SourceRequest,
    #[serde(default)]
    pub credentials: SourceCredentials,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_custom_key: Option<PartitionKey>,
}

pub async fn attach(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    request: SourceAttachRequest,
) -> Result<(LoadedChannel, SourceId), ManagementError> {
    observe(Operation::SourceAttach, async {
        let pointer = open_target(state, target)?;
        let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
        let current = load(state, creator, target).await?;
        allowed(state, &current, request.expected_version)?;
        custom_attachments_allowed(state, std::slice::from_ref(&request.source)).map_err(
            |error| match error {
                ManagementError::Invalid(error) => invalid(error, 0),
                error => error,
            },
        )?;
        let source = build_sources(std::slice::from_ref(&request.source), 8)
            .map_err(|error| invalid(error, 0))?
            .remove(0);
        let source_id = source.source_id();
        let mut next = current.definition.clone();
        next.secrets = source_secrets::merge(&current.definition, &request.credentials)
            .map_err(ManagementError::Invalid)?;
        validate_source_requirements(std::slice::from_ref(&source), &next.secrets)
            .map_err(|error| invalid(error, 0))?;
        partition(&mut next, &source, request.partition_custom_key)?;
        let index = next.sources.len();
        next.sources.push(source);
        next.validate().map_err(|error| invalid(error, index))?;
        write_gate::check(Some(&current.definition), &next, state.write_flags())
            .map_err(ManagementError::Invalid)?;
        let loaded = persist(state, current, next).await?;
        tracing::info!(
            record_id = %loaded.pointer.record_id, source_id = %source_id.as_str(),
            "channel source attached"
        );
        Ok((loaded, source_id))
    })
    .await
}

pub async fn detach(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    source_id: &SourceId,
    expected_version: u64,
) -> Result<LoadedChannel, ManagementError> {
    detach_named(state, creator, target, source_id.as_str(), expected_version).await
}

/// Raw HTTP names are classified only after ownership, version and write policy.
pub async fn detach_named(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    name: &str,
    expected_version: u64,
) -> Result<LoadedChannel, ManagementError> {
    observe(Operation::SourceDetach, async {
        let pointer = open_target(state, target)?;
        let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
        let current = load(state, creator, target).await?;
        allowed(state, &current, expected_version)?;
        let source_id = SourceId::parse(name).map_err(|_| ManagementError::SourceNotFound)?;
        let index = current
            .definition
            .sources
            .iter()
            .position(|source| source.source_id() == source_id)
            .ok_or(ManagementError::SourceNotFound)?;
        let protected = current
            .definition
            .routing
            .as_ref()
            .and_then(|routing| routing.default_reply.as_ref())
            .is_some_and(|reply| match reply {
                DefaultReply::TelegramChat { source_id: id, .. }
                | DefaultReply::RelayConversation { source_id: id, .. } => id == &source_id,
                DefaultReply::Sinks {} => false,
            });
        if protected {
            return Err(ManagementError::ConflictReason {
                detail: "default_reply_source_in_use",
                pointer: None,
            });
        }
        let mut next = current.definition.clone();
        if matches!(next.sources.remove(index), Source::Custom { .. }) {
            next.partition.custom_key = None;
        }
        source_secrets::prune(&mut next);
        next.validate().map_err(ManagementError::Invalid)?;
        write_gate::check(Some(&current.definition), &next, state.write_flags())
            .map_err(ManagementError::Invalid)?;
        let loaded = persist(state, current, next).await?;
        tracing::info!(
            record_id = %loaded.pointer.record_id, source_id = %source_id.as_str(),
            "channel source detached"
        );
        Ok(loaded)
    })
    .await
}

fn allowed(
    state: &ManagementState,
    current: &LoadedChannel,
    version: u64,
) -> Result<(), ManagementError> {
    check_version(current, version)?;
    if !state.routing.dynamic_sources_enabled {
        return Err(ManagementError::InvalidReason(
            write_gate::DYNAMIC_SOURCES_DISABLED,
        ));
    }
    if current.definition.connection_ref.is_some() {
        return Err(ManagementError::InvalidReason(
            "source_managed_by_connection",
        ));
    }
    Ok(())
}

fn partition(
    next: &mut ChannelDefinition,
    source: &Source,
    key: Option<PartitionKey>,
) -> Result<(), ManagementError> {
    let custom_chat = matches!(source, Source::Custom { .. })
        && matches!(
            next.partition.key,
            PartitionKey::Conversation | PartitionKey::ConversationAndSender | PartitionKey::Thread
        );
    if custom_chat {
        next.partition.custom_key = Some(key.ok_or_else(|| {
            ManagementError::Invalid(ValidationError::new(
                "/partition_custom_key",
                "required for a custom source on a chat-keyed channel",
            ))
        })?);
    } else if key.is_some() {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/partition_custom_key",
            "is accepted only for a custom source on a chat-keyed channel",
        )));
    }
    Ok(())
}

fn invalid(mut error: ValidationError, index: usize) -> ManagementError {
    let prefix = format!("/sources/{index}");
    if error.pointer == prefix {
        error.pointer = "/source".into();
    } else if let Some(suffix) = error.pointer.strip_prefix(&format!("{prefix}/")) {
        error.pointer = format!("/source/{suffix}");
    } else if let Some(suffix) = error.pointer.strip_prefix("/secrets/") {
        error.pointer = format!("/credentials/{suffix}");
    } else if error.pointer == "/partition/custom_key" {
        error.pointer = "/partition_custom_key".into();
    }
    ManagementError::Invalid(error)
}
