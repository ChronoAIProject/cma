use super::{codec, types::*};
use crate::ingress::{launch::LaunchRequest, routing_plan::RoutedPlan};
use cmaeg_core::{
    activity::ActivityKind,
    seal::{event_ref::activity_digest, MasterKeyRing},
};
use std::collections::BTreeSet;

/// An uncommitted, bounded upload. Reuse it unchanged after an uncertain write.
/// It carries sensitive source bytes and deliberately has no Debug formatter.
pub struct Candidate {
    pub(super) manifest: Manifest,
    pub(super) bytes: Vec<u8>,
    pub(super) event_expires_at: i64,
    pub(crate) profile_admission: Option<crate::channels::profile_assignment::Admission>,
}
impl Candidate {
    pub fn prepare(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Self, AdmissionError> {
        Self::prepare_projected(request, plan, source_body, keys, now, None)
    }
    pub(crate) fn prepare_projected(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
        quote_projection: Option<u8>,
    ) -> Result<Self, AdmissionError> {
        Self::prepare_inner(
            request,
            plan,
            source_body,
            keys,
            now,
            None,
            quote_projection,
            false,
            &BTreeSet::new(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_with_binding_preparation(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
        quote_projection: Option<u8>,
        replacements: &BTreeSet<String>,
    ) -> Result<Self, AdmissionError> {
        Self::prepare_inner(
            request,
            plan,
            source_body,
            keys,
            now,
            None,
            quote_projection,
            true,
            replacements,
        )
    }
    #[cfg(test)]
    pub(crate) fn prepare_native_interaction(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Self, AdmissionError> {
        Self::prepare_native_interaction_with_generation(
            request,
            plan,
            source_body,
            keys,
            now,
            false,
        )
    }
    pub(crate) fn prepare_native_interaction_with_generation(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
        generation_aware: bool,
    ) -> Result<Self, AdmissionError> {
        if crate::ingress::native_interaction::plan(request, keys, now).as_ref() != Ok(plan) {
            return Err(AdmissionError::Invalid);
        }
        Self::prepare_inner(
            request,
            plan,
            source_body,
            keys,
            now,
            Some(
                if crate::interactions::whatsapp_flow::shape(&request.activity) {
                    NATIVE_WHATSAPP_FLOW_INTERACTION_KIND
                } else if matches!(
                    request.activity.source,
                    cmaeg_core::activity::SourceKind::Whatsapp { .. }
                ) {
                    NATIVE_WHATSAPP_INTERACTION_KIND
                } else if matches!(
                    request.activity.source,
                    cmaeg_core::activity::SourceKind::NativeLark(_)
                ) {
                    NATIVE_LARK_INTERACTION_KIND
                } else {
                    NATIVE_INTERACTION_KIND
                },
            ),
            None,
            generation_aware,
            &BTreeSet::new(),
        )
    }
    #[cfg(test)]
    pub(crate) fn prepare_message_event(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Self, AdmissionError> {
        Self::prepare_message_event_with_generation(request, plan, source_body, keys, now, false)
    }
    pub(crate) fn prepare_message_event_with_generation(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
        generation_aware: bool,
    ) -> Result<Self, AdmissionError> {
        if !crate::ingress::message_event::shape(&request.activity) {
            return Err(AdmissionError::Invalid);
        }
        Self::prepare_inner(
            request,
            plan,
            source_body,
            keys,
            now,
            Some(MESSAGE_EVENT_KIND),
            None,
            generation_aware,
            &BTreeSet::new(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn prepare_inner(
        request: &LaunchRequest,
        plan: &RoutedPlan,
        source_body: &[u8],
        keys: &MasterKeyRing,
        now: i64,
        native_kind: Option<&str>,
        quote_projection: Option<u8>,
        generation_aware: bool,
        replacements: &BTreeSet<String>,
    ) -> Result<Self, AdmissionError> {
        let native = native_kind.is_some();
        if source_body.len() > MAX_SOURCE_BYTES {
            return Err(AdmissionError::TooLarge);
        }
        let original = request
            .event_ref
            .open(keys, now)
            .map_err(|error| match error {
                cmaeg_core::seal::OpenError::Expired => AdmissionError::Expired,
                _ => AdmissionError::Invalid,
            })?
            .payload;
        let mut incoming = plan.activity.clone();
        if !native {
            incoming.routing = None;
        }
        let identity = EventIdentity {
            owner: request.channel.definition.owner.subject.clone(),
            channel_id: request.channel.id.clone(),
            record_id: request.channel.pointer.record_id.clone(),
            event_id: request.activity.event_id.clone(),
        };
        identity.validate()?;
        if incoming != request.activity
            || (!native
                && (request.consumer.is_some()
                    || matches!(incoming.kind, ActivityKind::Interaction(_))
                    || original.consumer.is_some()))
            || original.lineage.is_some()
            || !original.verifies_activity(&incoming)
            || original.channel_id != identity.channel_id
            || original.record_id != identity.record_id
            || request.channel.definition.record_id != identity.record_id
            || original.conversation_id != request.conversation_id
            || request.channel.definition.source_of(&incoming.source) != Some(&plan.source)
            || plan.targets.iter().any(|target| {
                target.history_custody
                    != if native {
                        None
                    } else {
                        crate::native_lark::history_authority::admission_custody(
                            &request.channel,
                            &target.event,
                            &plan.activity,
                        )
                    }
                    || target.event.expires_at != original.expires_at
                    || target.event.reply != original.reply
                    || request
                        .channel
                        .consumer(Some(&target.consumer))
                        .is_none_or(|consumer| {
                            consumer.pending
                                || consumer.credential.is_none()
                                || consumer.provider != &target.provider
                        })
            })
        {
            return Err(AdmissionError::Invalid);
        }
        let admission_id = identity.admission_id();
        let mut payload = AdmittedPlan {
            kind: native_kind.unwrap_or(PAYLOAD_KIND).into(),
            admission_id: admission_id.clone(),
            identity,
            request_hash: codec::request_hash(&incoming, source_body)?,
            admitted_at: now,
            retained_until: now
                .checked_add(RETENTION_SECONDS)
                .ok_or(AdmissionError::Invalid)?,
            quote_projection,
            binding_preparation_version: generation_aware.then_some(1),
            binding_custom_source: None,
            activity: plan.activity.clone(),
            source: plan.source.clone(),
            targets: plan
                .targets
                .iter()
                .map(|launch| StoredTarget {
                    id: codec::identifier(
                        "ordinary-route-target-v1",
                        &(&admission_id, &launch.consumer),
                    ),
                    binding_replacement_version: (generation_aware
                        && !native
                        && super::preparation_source::eligible(&plan.activity, &plan.source)
                        && launch.provider.slug.as_str() == "cma_codex"
                        && launch.provider.target.is_profile()
                        && replacements.contains(launch.provider.binding_id.as_str()))
                    .then_some(1),
                    launch: launch.clone(),
                })
                .collect(),
            source_body: source_body.to_vec(),
        };
        if matches!(
            payload.activity.source,
            cmaeg_core::activity::SourceKind::Custom
        ) && payload
            .targets
            .iter()
            .any(|target| target.binding_replacement_version.is_some())
        {
            payload.binding_custom_source =
                Some(super::preparation_source::CustomSourceCustody::capture(
                    &payload.identity,
                    &payload.source,
                    &request.channel.definition,
                    keys,
                )?);
        }
        Self::from_payload(payload, original.expires_at)
    }
    pub(super) fn from_payload(
        payload: AdmittedPlan,
        event_expires_at: i64,
    ) -> Result<Self, AdmissionError> {
        let now = payload.admitted_at;
        payload.validate()?;
        let bytes = codec::canonical(&payload)?;
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(AdmissionError::TooLarge);
        }
        let manifest = Manifest {
            kind: ROOT_KIND.into(),
            id: payload.admission_id.clone(),
            identity: payload.identity,
            request_hash: payload.request_hash,
            payload_hash: codec::hash(&bytes),
            payload_len: bytes.len(),
            chunk_hashes: bytes.chunks(CHUNK_BYTES).map(codec::hash).collect(),
            admitted_at: now,
            retained_until: payload.retained_until,
        };
        manifest.validate(now)?;
        codec::encode(&manifest)?;
        Ok(Self {
            manifest,
            bytes,
            event_expires_at,
            profile_admission: None,
        })
    }
    pub fn identity(&self) -> &EventIdentity {
        &self.manifest.identity
    }
    pub fn request_hash(&self) -> &str {
        &self.manifest.request_hash
    }
    pub fn admission_id(&self) -> &str {
        &self.manifest.id
    }
    pub(super) fn live(&self, now: i64) -> Result<(), AdmissionError> {
        self.manifest.validate(now)?;
        if self.event_expires_at <= now {
            return Err(AdmissionError::Expired);
        }
        Ok(())
    }
}

impl AdmittedPlan {
    pub(super) fn validate(&self) -> Result<(), AdmissionError> {
        if self.is_compatibility() {
            return self.validate_compatibility();
        }
        self.identity.validate()?;
        let routing = self
            .activity
            .routing
            .as_ref()
            .ok_or(AdmissionError::Corrupt)?;
        let discord_native = self.kind == NATIVE_INTERACTION_KIND;
        let whatsapp_native = self.kind == NATIVE_WHATSAPP_INTERACTION_KIND;
        let lark_native = self.kind == NATIVE_LARK_INTERACTION_KIND;
        let flow_native = self.kind == NATIVE_WHATSAPP_FLOW_INTERACTION_KIND;
        let native = discord_native || whatsapp_native || lark_native || flow_native;
        let message_event = self.kind == MESSAGE_EVENT_KIND;
        if !self.valid_custom_source_custody()
            || self.quote_projection.is_some_and(|v| v != 1)
            || self.binding_preparation_version.is_some_and(|v| v != 1)
            || (self.quote_projection.is_some()
                && (self.kind != PAYLOAD_KIND
                    || !matches!(self.activity.kind, ActivityKind::Message { .. })))
            || (self.quote_projection.is_none()
                && matches!(
                    &self.activity.kind,
                    ActivityKind::Message {
                        quoted_message: Some(_),
                        ..
                    }
                ))
            || (self.kind != PAYLOAD_KIND && !native && !message_event)
            || self.admission_id != self.identity.admission_id()
            || self.activity.validate().is_err()
            || if native {
                !crate::ingress::native_interaction::shape(&self.activity)
                    || !(discord_native
                        && matches!(self.source, cmaeg_core::channel::Source::Discord { .. })
                        || whatsapp_native
                            && matches!(self.source, cmaeg_core::channel::Source::Whatsapp { .. })
                        || flow_native
                            && crate::interactions::whatsapp_flow::shape(&self.activity)
                            && matches!(self.source, cmaeg_core::channel::Source::Whatsapp { .. })
                        || lark_native
                            && matches!(self.source, cmaeg_core::channel::Source::NativeLark(_)))
            } else if message_event {
                !crate::ingress::message_event::shape(&self.activity)
            } else {
                matches!(self.activity.kind, ActivityKind::Interaction(_))
            }
            || self.activity.event_id != self.identity.event_id
            || self.targets.len() != routing.targets.len()
            || self.targets.is_empty()
            || self.targets.len() > usize::from(cmaeg_core::channel::MAX_TARGETS)
            || self.source_body.len() > MAX_SOURCE_BYTES
            || codec::request_hash(&self.activity, &self.source_body)? != self.request_hash
        {
            return Err(AdmissionError::Corrupt);
        }
        let mut seen = BTreeSet::new();
        for (target, selected) in self.targets.iter().zip(&routing.targets) {
            let launch = &target.launch;
            let event = &launch.event;
            let expected_consumer = (!launch.consumer.is_default()).then_some(&launch.consumer);
            if target.binding_replacement_version.is_some_and(|v| v != 1)
                || (target.binding_replacement_version.is_some()
                    && (!self.prepares_binding(target) || !self.permits_binding_preparation()))
                || target.id
                    != codec::identifier(
                        "ordinary-route-target-v1",
                        &(&self.admission_id, &launch.consumer),
                    )
                || !seen.insert(&target.id)
                || launch.consumer != selected.id
                || event.consumer.as_ref() != expected_consumer
                || event.lineage.is_some()
                || event.validate().is_err()
                || event.channel_id != self.identity.channel_id
                || event.record_id != self.identity.record_id
                || event.event_id != self.identity.event_id
                || event.activity_sha256.as_ref() != Some(&activity_digest(&self.activity))
                || event.expires_at <= self.admitted_at
                || event.expires_at > self.retained_until
            {
                return Err(AdmissionError::Corrupt);
            }
        }
        Ok(())
    }
}
