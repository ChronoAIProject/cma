//! Preserve an authenticated legacy launch as one durable target, without routing.
use super::{codec, types::*, Candidate, PlanStore};
use crate::ingress::{
    launch::LaunchRequest,
    routing_plan::{ConsumerLaunch, RoutedPlan},
};
use cmaeg_core::{
    activity::SourceKind,
    channel::Source,
    seal::{EventRefPayload, MasterKeyRing},
};

impl Candidate {
    pub(crate) fn compatibility_delivery_plan(&self) -> Result<RoutedPlan, AdmissionError> {
        let plan: AdmittedPlan = codec::decode(&self.bytes)?;
        plan.validate_compatibility()?;
        Ok(plan.compatibility_delivery_plan())
    }

    pub(crate) fn prepare_compatibility(
        request: &LaunchRequest,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Self, AdmissionError> {
        let (identity, event) = verified(request, source_body, keys, now)?;
        let source = request
            .channel
            .definition
            .source_of(&request.activity.source)
            .ok_or(AdmissionError::Invalid)?
            .clone();
        if let (SourceKind::NyxidRelay { route_id, .. }, Source::NyxidRelay { route_ids, .. }) =
            (&request.activity.source, &source)
        {
            if !route_ids.contains(route_id) {
                return Err(AdmissionError::Invalid);
            }
        }
        let consumer = request
            .channel
            .consumer(request.consumer.as_ref())
            .filter(|c| !c.pending && c.credential.is_some())
            .ok_or(AdmissionError::Invalid)?;
        let admission_id = identity.admission_id();
        let target = StoredTarget {
            id: codec::identifier("ordinary-route-target-v1", &(&admission_id, consumer.id)),
            launch: ConsumerLaunch {
                history_custody: crate::native_lark::history_authority::admission_custody(
                    &request.channel,
                    &event,
                    &request.activity,
                ),
                consumer: consumer.id.clone(),
                provider: consumer.provider.clone(),
                idempotency_key: request.idempotency_key.clone(),
                event: event.clone(),
            },
            binding_replacement_version: None,
        };
        Self::from_payload(
            AdmittedPlan {
                kind: COMPATIBILITY_KIND.into(),
                admission_id,
                identity,
                request_hash: codec::request_hash(&request.activity, source_body)?,
                admitted_at: now,
                retained_until: now
                    .checked_add(RETENTION_SECONDS)
                    .ok_or(AdmissionError::Invalid)?,
                quote_projection: None,
                binding_preparation_version: None,
                binding_custom_source: None,
                activity: request.activity.clone(),
                source,
                targets: vec![target],
                source_body: source_body.to_vec(),
            },
            event.expires_at,
        )
    }
}

impl AdmittedPlan {
    pub(crate) fn is_compatibility(&self) -> bool {
        self.kind == COMPATIBILITY_KIND
    }

    pub(super) fn validate_compatibility(&self) -> Result<(), AdmissionError> {
        self.identity.validate()?;
        let [target] = self.targets.as_slice() else {
            return Err(AdmissionError::Corrupt);
        };
        let launch = &target.launch;
        let event = &launch.event;
        if self.admission_id != self.identity.admission_id()
            || self.activity.validate().is_err()
            || self.activity.event_id != self.identity.event_id
            || self.quote_projection.is_some()
            || self.binding_preparation_version.is_some()
            || self.binding_custom_source.is_some()
            || target.binding_replacement_version.is_some()
            || target.id
                != codec::identifier(
                    "ordinary-route-target-v1",
                    &(&self.admission_id, &launch.consumer),
                )
            || event.validate().is_err()
            || !event.verifies_activity(&self.activity)
            || event.channel_id != self.identity.channel_id
            || event.record_id != self.identity.record_id
            || event.consumer.as_ref()
                != (!launch.consumer.is_default()).then_some(&launch.consumer)
            || event.expires_at <= self.admitted_at
            || event.expires_at > self.retained_until
            || self.source_body.len() > MAX_SOURCE_BYTES
            || self.request_hash != codec::request_hash(&self.activity, &self.source_body)?
        {
            return Err(AdmissionError::Corrupt);
        }
        Ok(())
    }

    /// A validation view only: no new routing policy or event reference is minted.
    pub(crate) fn compatibility_delivery_plan(&self) -> RoutedPlan {
        RoutedPlan {
            activity: self.activity.clone(),
            source: self.source.clone(),
            targets: self.targets.iter().map(|t| t.launch.clone()).collect(),
            trace: Default::default(),
        }
    }

    pub(super) fn check_compatibility_replay(
        &self,
        request: &LaunchRequest,
        event: &EventRefPayload,
    ) -> Result<(), AdmissionError> {
        if self.is_compatibility() {
            let [target] = self.targets.as_slice() else {
                return Err(AdmissionError::Corrupt);
            };
            let retained = &target.launch;
            if retained.idempotency_key != request.idempotency_key
                || retained.event.consumer != event.consumer
                || retained.event.conversation_id != event.conversation_id
                || retained.event.partition_key != event.partition_key
                || retained.event.lineage != event.lineage
            {
                return Err(AdmissionError::Conflict);
            }
        }
        Ok(())
    }
}

impl PlanStore {
    pub(crate) fn replicas_enabled(&self) -> bool {
        self.admission_queue.is_some()
    }

    pub(crate) async fn replay_compatibility(
        &self,
        request: &LaunchRequest,
        source_body: &[u8],
        profile_guard: bool,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        if !self.replicas_enabled() && !profile_guard {
            return Err(AdmissionError::Invalid);
        }
        let (identity, event) = verified(request, source_body, &self.keys, (self.clock)())?;
        let Some(plan) = self.load(&identity).await? else {
            return Ok(None);
        };
        if plan.request_hash != codec::request_hash(&request.activity, source_body)? {
            return Err(AdmissionError::Conflict);
        }
        plan.check_compatibility_replay(request, &event)?;
        self.queue_committed(plan).await.map(Some)
    }

    pub(crate) async fn admit_compatibility(
        &self,
        candidate: &Candidate,
        request: &LaunchRequest,
    ) -> Result<AdmittedPlan, AdmissionError> {
        if (!self.replicas_enabled() && candidate.profile_admission.is_none())
            || matches!(request.activity.source, SourceKind::Github { .. })
        {
            return Err(AdmissionError::Invalid);
        }
        self.admit(candidate).await
    }
}

fn verified(
    request: &LaunchRequest,
    source_body: &[u8],
    keys: &MasterKeyRing,
    now: i64,
) -> Result<(EventIdentity, EventRefPayload), AdmissionError> {
    if source_body.len() > MAX_SOURCE_BYTES {
        return Err(AdmissionError::TooLarge);
    }
    let event = request
        .event_ref
        .open(keys, now)
        .map_err(|e| match e {
            cmaeg_core::seal::OpenError::Expired => AdmissionError::Expired,
            _ => AdmissionError::Invalid,
        })?
        .payload;
    let identity = EventIdentity {
        owner: request.channel.definition.owner.subject.clone(),
        channel_id: request.channel.id.clone(),
        record_id: request.channel.pointer.record_id.clone(),
        event_id: request.activity.event_id.clone(),
    };
    identity.validate()?;
    if request.activity.validate().is_err()
        || !event.verifies_activity(&request.activity)
        || event.channel_id != identity.channel_id
        || event.record_id != identity.record_id
        || request.channel.definition.record_id != identity.record_id
        || event.conversation_id != request.conversation_id
        || event.consumer != request.consumer
    {
        return Err(AdmissionError::Invalid);
    }
    Ok((identity, event))
}
