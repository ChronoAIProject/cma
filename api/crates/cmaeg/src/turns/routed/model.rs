use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    providers::EventContextSnapshot,
};
use cmaeg_core::{
    channel::{ReplyChoice, ReplyPolicy},
    ids::{ConsumerId, ConversationId, IdempotencyKey},
    seal::{EventRef, MasterKeyRing, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const KIND: &str = "ordinary_route_execution_v1";
pub(super) const BODY_KIND: &str = "ordinary_route_execution_body_v1";
pub(super) const MAX_BODY: usize = 1024 * 1024;
pub(super) const BODY_CHUNK: usize = 24 * 1024;
pub(super) const LEASE_SECONDS: i64 = 30;

pub(super) fn settlement_requires_live_event_ref(plan: &AdmittedPlan) -> bool {
    !matches!(
        plan.activity.source,
        cmaeg_core::activity::SourceKind::Custom | cmaeg_core::activity::SourceKind::Github { .. }
    )
}

pub use cmaeg_engine::{ExecutionError, Phase, RecoverySupport};

/// Exact provider body fields. Credentials are injected only at the call site.
/// No Debug implementation: input and event context can contain private data.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenTurn {
    pub conversation_id: ConversationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_generation: Option<u64>,
    pub idempotency_key: IdempotencyKey,
    pub input: Vec<Value>,
    pub metadata: BTreeMap<String, String>,
    pub event_context: EventContextSnapshot,
    pub delivery: FrozenDelivery,
}

/// Reply presentation and policy are frozen with the provider request. The
/// event reference carries origin authority; a default destination is bound
/// separately without credentials. Current authorization is required to send.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenDelivery {
    pub consumer: ConsumerId,
    pub label: String,
    pub auto_final: bool,
    pub destination: ReplyChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_destination: Option<super::DefaultDestination>,
    pub policy: ReplyPolicy,
}
impl FrozenTurn {
    /// The canonical Responses request bytes, excluding the credential and the
    /// separately transmitted idempotency key. Adapters must send these fields
    /// unchanged; delivery policy is a local journal field, never provider input.
    pub fn provider_body(&self) -> Vec<u8> {
        let mut body = serde_json::json!({
            "conversation":self.conversation_id,"stream":true,"input":self.input,
            "metadata":self.metadata,"event_context":self.event_context,
        });
        if let Some(generation) = self.binding_generation {
            body["binding_generation"] = generation.into();
        }
        cmaeg_core::profile::canonical_json(body).into_bytes()
    }
    pub fn event_ref(
        plan: &AdmittedPlan,
        target: &StoredTarget,
        keys: &MasterKeyRing,
    ) -> Result<EventRef, ExecutionError> {
        EventRef::seal(&delivery_event(plan, target), keys).map_err(|_| ExecutionError::Invalid)
    }
    pub(super) fn validate(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<(), ExecutionError> {
        let opened = self
            .event_context
            .event_ref
            .open(keys, now)
            .map_err(|_| ExecutionError::Expired)?
            .payload;
        self.validate_event(plan, target, &opened)
    }

    /// Verify retained execution metadata, not a live reply/tool capability.
    /// Accepted automation may finish after the reference's delivery deadline.
    pub(super) fn settlement_event(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<cmaeg_core::seal::EventRefPayload, ExecutionError> {
        let opened = if !settlement_requires_live_event_ref(plan) {
            cmaeg_core::seal::open::<cmaeg_core::seal::EventRefPayload>(
                keys,
                cmaeg_core::seal::TokenKind::EventRef,
                self.event_context.event_ref.as_str(),
            )
            .map_err(|_| ExecutionError::Expired)?
            .payload
        } else {
            self.event_context
                .event_ref
                .open(keys, now)
                .map_err(|_| ExecutionError::Expired)?
                .payload
        };
        opened.validate().map_err(|_| ExecutionError::Invalid)?;
        self.validate_event(plan, target, &opened)?;
        Ok(opened)
    }

    pub(super) fn validate_settlement(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<(), ExecutionError> {
        self.settlement_event(plan, target, keys, now).map(|_| ())
    }

    fn validate_event(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        opened: &cmaeg_core::seal::EventRefPayload,
    ) -> Result<(), ExecutionError> {
        if self.conversation_id != target.launch.event.conversation_id
            || self.binding_generation == Some(0)
            || (self.binding_generation.is_some() && !plan.prepares_binding(target))
            || self.idempotency_key != target.launch.idempotency_key
            || self.event_context.channel_id != plan.identity.channel_id
            || self.event_context.conversation_id != self.conversation_id
            || self.event_context.activity != plan.activity
            || self.delivery.consumer != target.launch.consumer
            || self.delivery.label.is_empty()
            || self.delivery.label.len() > 256
            || (self.delivery.destination == ReplyChoice::Default)
                != self.delivery.default_destination.is_some()
            || opened != &delivery_event(plan, target)
            || self.event_context.validate().is_err()
            || self.input.is_empty()
            || serde_json::to_vec(&self.input)
                .map_err(|_| ExecutionError::Invalid)?
                .len()
                > 256 * 1024
            || self.metadata.len() > 16
            || self
                .metadata
                .iter()
                .any(|(key, value)| key.len() > 64 || value.len() > 512)
        {
            return Err(ExecutionError::Invalid);
        }
        Ok(())
    }
}
pub(super) fn delivery_event(
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> cmaeg_core::seal::EventRefPayload {
    let mut event = target.launch.event.clone();
    if plan
        .targets
        .first()
        .is_some_and(|primary| primary.id != target.id)
        && matches!(event.reply, ReplyAddress::Sync { .. })
    {
        event.reply = ReplyAddress::None;
    }
    event
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BodyReference {
    pub hash: String,
    pub len: usize,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ownership {
    pub worker: String,
    pub claim: String,
    pub epoch: u64,
    pub until: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetState {
    pub(super) kind: String,
    pub id: String,
    pub root: String,
    pub owner: String,
    pub request_hash: String,
    pub retained_until: i64,
    pub event_expires_at: i64,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_preparation: Option<super::BindingPreparation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<super::delivery_state::DeliveryCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) projection: Option<super::projection_store::ProjectionReference>,
    pub(super) body: Option<BodyReference>,
    pub(super) ownership: Ownership,
    #[serde(skip)]
    pub version: u64,
}
impl std::fmt::Debug for TargetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetState")
            .field("id", &self.id)
            .field("phase", &self.phase)
            .field("version", &self.version)
            .finish()
    }
}

/// An opaque local ownership receipt. Every mutation rechecks its CAS version.
#[derive(Clone)]
pub struct Lease {
    pub(super) state: TargetState,
    pub(super) partition: Option<crate::replicas::partition::PartitionLease>,
}
impl Lease {
    pub fn state(&self) -> &TargetState {
        &self.state
    }
}
