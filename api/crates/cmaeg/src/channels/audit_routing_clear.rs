//! The `--clear` path of the routing audit: return a definition to the
//! legacy byte shape, or leave it for its owner.
//!
//! Clearing must never silently widen who sees events. Removing `routing`
//! sends every event to `default`, so a channel whose policy keeps some
//! events from `default` (by rules, by explicit addressing or because
//! `default` does not answer non-human actors) is left unchanged unless the
//! operator passes the widening override with the owner's sign-off. Source
//! shapes that only an owner can reduce are never touched.
//!
//! `--clear` runs as a separate process from the serving gateway, so it
//! cannot invalidate a replica's channel cache. A serving replica may keep
//! using its cached definition until `CMAEG_CHANNEL_CACHE_TTL_MS` plus the
//! equally long stale window used when a reload fails have expired. With the
//! multi-consumer flags off, as the rollback order requires before clearing,
//! the per-event kill switch still applies to that cached policy: it can keep
//! an event from a consumer, but never send it to one other than `default`.
use super::{
    audit::{AuditError, AuditReport},
    audit_pages::{rewrite, Rewritten, Step},
    audit_routing::{RoutingAuditAction, RoutingFinding},
};
use crate::{
    providers::{ProviderClient, ProviderError},
    records::{RecordClient, RecordHead},
};
use cmaeg_core::{
    channel::{default_receives_all, ChannelDefinition, Extensions, Secret, Source},
    ids::{BindingId, ConsumerId, ProviderSlug},
    seal::MasterKeyRing,
};
use std::collections::BTreeSet;

/// Extended sources remain once `custom_key` is gone: 0 or more than 4
/// sources, several relays, a custom source on a chat-keyed channel, or raw-event
/// verification whose removal would change the owner-selected trust policy.
pub const SOURCES_NEED_OWNER: &str = "sources_need_owner";
/// Removing routing would let `default` see events its policy keeps from it.
pub const DEFAULT_WOULD_WIDEN: &str = "default_would_widen";
/// A provider refused to remove an additional consumer's binding.
pub const UNBIND_FAILED: &str = "unbind_failed";
/// The record changed again after one re-read.
pub const CONFLICT: &str = "conflict";
/// Execution-mode authority cannot be erased by a topology rollback.
pub const CUSTOM_EXECUTION_NEEDS_READER: &str = "durable_custom_execution_requires_reader";

/// Everything a clearing run reads besides the record itself.
pub(super) struct Clearing<'a> {
    pub(super) records: &'a RecordClient,
    pub(super) keys: &'a MasterKeyRing,
    pub(super) providers: Option<&'a ProviderClient>,
    pub(super) non_human: &'a BTreeSet<String>,
    pub(super) allow_default_widening: bool,
}

/// How clearing one definition ended.
pub(super) struct Outcome {
    pub(super) action: RoutingAuditAction,
    pub(super) reason: Option<&'static str>,
    /// Additional consumer bindings removed, 404s included.
    pub(super) unbound: usize,
    /// Whether the widening override was needed to clear it.
    pub(super) widened: bool,
}

/// One additional consumer's binding, collected before the definition that
/// holds its key is consumed.
struct Binding {
    slug: ProviderSlug,
    binding_id: BindingId,
    consumer: ConsumerId,
    credential: Option<Secret>,
}

/// Clear one definition: block it when it needs an owner decision, else
/// remove every additional consumer's binding and rewrite it in the legacy
/// shape under compare-and-swap. After a version conflict the stored
/// definition is re-read and classified again; a binding already removed is
/// not removed twice.
pub(super) async fn clear(
    clearing: &Clearing<'_>,
    head: &RecordHead,
    definition: ChannelDefinition,
    version: u64,
    report: &mut AuditReport<RoutingFinding>,
) -> Result<Outcome, AuditError> {
    let mut unbound = BTreeSet::new();
    let mut widened = false;
    let rewritten = rewrite(
        clearing.records,
        clearing.keys,
        head,
        definition,
        version,
        report,
        async |definition| step(clearing, definition, &mut unbound, &mut widened).await,
    )
    .await?;
    let (action, reason) = match rewritten {
        Rewritten::Cleared => (RoutingAuditAction::Cleared, None),
        Rewritten::Stopped(reason) => (RoutingAuditAction::Blocked, Some(reason)),
        Rewritten::Conflict => (RoutingAuditAction::Conflict, Some(CONFLICT)),
    };
    Ok(Outcome {
        action,
        reason,
        unbound: unbound.len(),
        widened,
    })
}

/// Decide what to store in place of `definition`, in the documented order:
/// blocked shapes first, then unbinding, then the legacy shape to write.
async fn step(
    clearing: &Clearing<'_>,
    definition: ChannelDefinition,
    unbound: &mut BTreeSet<(ProviderSlug, BindingId)>,
    widened: &mut bool,
) -> Result<Step<&'static str>, AuditError> {
    // Decided afresh for whatever is stored now, as every other outcome is.
    *widened = false;
    if definition.durable_custom_execution {
        return Ok(Step::Stop(CUSTOM_EXECUTION_NEEDS_READER));
    }
    if Extensions::of(&definition).provider_capabilities {
        // Negotiation belongs to the binding transaction. A routing-only
        // rollback cannot erase it while the provider may still emit fields.
        return Ok(Step::Stop("provider_capabilities_require_reader"));
    }
    if Extensions::of(&definition).legacy_compatible() {
        // A concurrent update already returned it to the legacy shape.
        return Ok(Step::Done);
    }
    let widens =
        definition.routing.is_some() && !default_receives_all(&definition, clearing.non_human);
    let bindings = additional_bindings(&definition);
    let next = legacy_shape(definition);
    // Raw-event verification remains owner-controlled. Clearing it could
    // either admit unverified facts or disable a working source. Neither is
    // a safe automatic rollback, even with the routing-widening override.
    if Extensions::of(&next).sources {
        return Ok(Step::Stop(SOURCES_NEED_OWNER));
    }
    if widens && !clearing.allow_default_widening {
        return Ok(Step::Stop(DEFAULT_WOULD_WIDEN));
    }
    *widened = widens;
    for binding in bindings {
        let key = (binding.slug.clone(), binding.binding_id.clone());
        if unbound.contains(&key) {
            continue;
        }
        if !unbind(clearing.providers, &binding).await? {
            return Ok(Step::Stop(UNBIND_FAILED));
        }
        unbound.insert(key);
    }
    Ok(Step::Write(Box::new(next)))
}

/// Every additional consumer's binding, pending ones included: a pending
/// consumer's binding may exist when its creation stopped part way, and a
/// provider answers 404 for one that does not.
fn additional_bindings(definition: &ChannelDefinition) -> Vec<Binding> {
    definition
        .consumers()
        .skip(1)
        .map(|consumer| Binding {
            slug: consumer.provider.slug.clone(),
            binding_id: consumer.provider.binding_id.clone(),
            consumer: consumer.id.clone(),
            credential: consumer.credential.cloned(),
        })
        .collect()
}

/// The definition without native opt-ins, routing and consumer keys, and without a
/// `custom_key` that no custom source uses.
fn legacy_shape(mut definition: ChannelDefinition) -> ChannelDefinition {
    for source in &mut definition.sources {
        if let Source::Telegram { admission, .. }
        | Source::Discord { admission, .. }
        | Source::NyxidRelay { admission, .. } = source
        {
            if let cmaeg_core::channel::AdmissionPolicy::Scoped { bot_messages, .. } = admission {
                *bot_messages = Default::default();
            }
        }
    }
    definition.routing = None;
    definition.reply.thread_contract = None;
    definition.secrets.consumer_keys.clear();
    if !definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::Custom { .. }))
    {
        definition.partition.custom_key = None;
    }
    definition
}

/// Remove one consumer's binding through the same call channel deletion uses
/// for `default`. A 404 counts as removed. Returns whether it was removed;
/// the key itself is never logged.
async fn unbind(providers: Option<&ProviderClient>, binding: &Binding) -> Result<bool, AuditError> {
    let providers = providers.ok_or(AuditError::ProviderConfiguration)?;
    let Some(credential) = &binding.credential else {
        // The reader requires one key per consumer, so this is a defect.
        tracing::warn!(
            provider = %binding.slug,
            consumer = binding.consumer.as_str(),
            binding_id = binding.binding_id.as_str(),
            "consumer has no stored key; its binding cannot be removed"
        );
        return Ok(false);
    };
    match providers
        .unbind(&binding.slug, &binding.binding_id, credential)
        .await
    {
        Ok(()) => {
            tracing::info!(
                provider = %binding.slug,
                consumer = binding.consumer.as_str(),
                binding_id = binding.binding_id.as_str(),
                "consumer binding removed"
            );
            Ok(true)
        }
        Err(error) => {
            tracing::warn!(
                provider = %binding.slug,
                consumer = binding.consumer.as_str(),
                binding_id = binding.binding_id.as_str(),
                failure = failure(&error),
                "consumer binding could not be removed; channel left unchanged"
            );
            Ok(false)
        }
    }
}

/// A provider failure's class: its details can carry an upstream body.
fn failure(error: &ProviderError) -> String {
    match error {
        ProviderError::UnknownProvider(_) => "unknown_provider".into(),
        ProviderError::InvalidRequest(_) => "invalid_request".into(),
        ProviderError::Busy { .. } => "busy".into(),
        ProviderError::Capacity => "capacity".into(),
        ProviderError::ConversationMissing => "conversation_missing".into(),
        ProviderError::Rejected { status, .. } => format!("rejected_{status}"),
        ProviderError::Unavailable(_) => "unavailable".into(),
        ProviderError::ResponseMalformed(_) => "response_malformed".into(),
        ProviderError::Sse(_) => "sse".into(),
    }
}
