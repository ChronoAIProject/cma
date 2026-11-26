use super::{codec, types::*, Candidate};
use crate::ingress::admission_trace::{self as trace, Stage};
use crate::records::{AuthorityGuard, RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct PlanStore {
    pub(crate) start_measurement: Option<crate::telemetry::provider_start::Measurement>,
    pub(super) records: RecordClient,
    pub(super) keys: Arc<MasterKeyRing>,
    pub(super) clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub(super) admission_queue: Option<Arc<dyn super::AdmissionQueue>>,
}
impl PlanStore {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            admission_queue: None,
            start_measurement: None,
            records: records.interactions(),
            keys,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |time| time.as_secs().try_into().unwrap_or(i64::MAX))
            }),
        }
    }

    pub fn with_start_measurement(
        mut self,
        measurement: crate::telemetry::provider_start::Measurement,
    ) -> Self {
        self.start_measurement = Some(measurement);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    /// Check an admitted event before consulting current routing policy. A
    /// duplicate still restores its snapshot after routing/consumers are removed.
    /// Ingress authentication and source admission remain the caller's duty.
    pub async fn replay(
        &self,
        request: &crate::ingress::launch::LaunchRequest,
        source_body: &[u8],
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        self.replay_inner(request, source_body, false).await
    }
    pub(crate) async fn replay_native_interaction(
        &self,
        request: &crate::ingress::launch::LaunchRequest,
        source_body: &[u8],
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        if !crate::ingress::native_interaction::shape(&request.activity) {
            return Err(AdmissionError::Invalid);
        }
        self.replay_inner(request, source_body, true).await
    }
    pub(crate) async fn replay_message_event(
        &self,
        request: &crate::ingress::launch::LaunchRequest,
        source_body: &[u8],
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        if !crate::ingress::message_event::shape(&request.activity) {
            return Err(AdmissionError::Invalid);
        }
        self.replay_inner(request, source_body, true).await
    }
    async fn replay_inner(
        &self,
        request: &crate::ingress::launch::LaunchRequest,
        source_body: &[u8],
        native: bool,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        if source_body.len() > MAX_SOURCE_BYTES {
            return Err(AdmissionError::TooLarge);
        }
        if !native
            && (request.consumer.is_some()
                || request.activity.routing.is_some()
                || matches!(
                    request.activity.kind,
                    cmaeg_core::activity::ActivityKind::Interaction(_)
                ))
        {
            return Err(AdmissionError::Invalid);
        }
        let event = request
            .event_ref
            .open(&self.keys, (self.clock)())
            .map_err(|error| match error {
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
        if (!native && event.consumer.is_some())
            || (native && event.consumer != request.consumer)
            || event.lineage.is_some()
            || !event.verifies_activity(&request.activity)
            || event.channel_id != identity.channel_id
            || event.record_id != identity.record_id
            || request.channel.definition.record_id != identity.record_id
            || event.conversation_id != request.conversation_id
        {
            return Err(AdmissionError::Invalid);
        }
        let request_hash = codec::request_hash(&request.activity, source_body)?;
        let restored = self.load(&identity).await?;
        if restored
            .as_ref()
            .is_some_and(|plan| plan.request_hash != request_hash)
        {
            return Err(AdmissionError::Conflict);
        }
        if let Some(plan) = &restored {
            plan.check_compatibility_replay(request, &event)?;
        }
        match restored {
            Some(plan) => self.queue_committed(plan).await.map(Some),
            None => Ok(None),
        }
    }

    /// Returns after the root and payload verify and any installed admission
    /// queue acknowledges every target. Queue failure retains the original root.
    /// A duplicate logical request restores the original plan despite policy edits.
    pub async fn admit(&self, candidate: &Candidate) -> Result<AdmittedPlan, AdmissionError> {
        self.admit_inner(candidate, None).await
    }

    /// GitHub-only admission root. The channel version is guarded in the same
    /// Mongo transaction that inserts the immutable interaction root.
    pub(crate) async fn admit_github(
        &self,
        candidate: &Candidate,
        channel_record_id: &str,
        channel_version: u64,
        authority_guards: &[AuthorityGuard],
    ) -> Result<AdmittedPlan, AdmissionError> {
        self.admit_inner(
            candidate,
            Some((
                channel_record_id.to_owned(),
                channel_version,
                authority_guards.to_vec(),
            )),
        )
        .await
    }

    async fn admit_inner(
        &self,
        candidate: &Candidate,
        github_guard: Option<(String, u64, Vec<AuthorityGuard>)>,
    ) -> Result<AdmittedPlan, AdmissionError> {
        if let Some(measurement) = &self.start_measurement {
            measurement.offered(&candidate.identity().admission_id());
        }
        candidate.live((self.clock)())?;
        if let Some(plan) = self.load(candidate.identity()).await? {
            let plan = same_request(candidate, plan)?;
            if let Some(measurement) = &self.start_measurement {
                measurement.admitted(
                    &plan.admission_id,
                    plan.admitted_at,
                    plan.targets.len(),
                    None,
                );
            }
            return self.queue_committed(plan).await;
        }
        for (index, chunk) in candidate.bytes.chunks(CHUNK_BYTES).enumerate() {
            candidate.live((self.clock)())?;
            let bytes = codec::chunk_bytes(&candidate.manifest, index, chunk)?;
            trace::admission(
                Stage::ChunkWrite,
                self.create_immutable(
                    &candidate.manifest.chunk_id(index),
                    &candidate.manifest,
                    &bytes,
                ),
            )
            .await?;
        }
        candidate.live((self.clock)())?;
        let manifest = &candidate.manifest;
        let bytes = codec::encode(manifest)?;
        let commit_lower = self.start_measurement.as_ref().map(|m| m.now());
        let result = trace::record(
            Stage::RootWrite,
            self.create_root(
                manifest,
                &bytes,
                github_guard,
                candidate.profile_admission.as_ref(),
            ),
        )
        .await;
        if matches!(&result, Err(RecordError::GuardConflict { .. })) {
            return Err(AdmissionError::GuardConflict);
        }
        match result {
            Ok(()) | Err(RecordError::Conflict { .. }) | Err(RecordError::Unavailable(_)) => {
                // A lost response is resolved by reading the immutable root. A
                // competing same-body candidate may have a different routing plan.
                let readback = async {
                    if matches!(result, Err(RecordError::Conflict { .. })) {
                        self.load_conflicting_root(&manifest.identity).await
                    } else {
                        self.load(&manifest.identity).await
                    }
                };
                match trace::admission(Stage::RootReadback, readback).await {
                    Ok(Some(plan)) => {
                        let plan = same_request(candidate, plan)?;
                        if let Some(measurement) = &self.start_measurement {
                            measurement.admitted(
                                &plan.admission_id,
                                plan.admitted_at,
                                plan.targets.len(),
                                if result.is_ok() { commit_lower } else { None },
                            );
                        }
                        self.queue_committed(plan).await
                    }
                    Ok(None) if matches!(result, Err(RecordError::Conflict { .. })) => {
                        Err(AdmissionError::ExpiredOrUnavailable)
                    }
                    Ok(None) | Err(AdmissionError::Unavailable) => Err(AdmissionError::Uncertain),
                    Err(error) => Err(error),
                }
            }
            Err(_) => Err(AdmissionError::Unavailable),
        }
    }

    /// A retained record can be read after event authority expires. This never
    /// renews its event references or authorizes a new provider turn.
    pub async fn load(
        &self,
        identity: &EventIdentity,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        trace::admission(Stage::RootLookup, self.load_observed(identity)).await
    }

    /// A duplicate-key conflict can precede the winner's majority visibility.
    /// Reconcile that exact immutable root without repeating any write or effect.
    async fn load_conflicting_root(
        &self,
        identity: &EventIdentity,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        crate::readback::bounded(|| self.load(identity), || AdmissionError::Unavailable).await
    }

    async fn load_observed(
        &self,
        identity: &EventIdentity,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        identity.validate()?;
        let id = identity.admission_id();
        let Some(bytes) = self.read(&id).await? else {
            return Ok(None);
        };
        let manifest: Manifest = codec::decode(&bytes)?;
        manifest.validate((self.clock)())?;
        if manifest.identity != *identity || manifest.id != id {
            return Err(AdmissionError::Corrupt);
        }
        self.restore(&manifest).await.map(Some)
    }

    pub async fn inspect(&self, candidate: &Candidate) -> Result<AdmissionState, AdmissionError> {
        candidate.live((self.clock)())?;
        if let Some(plan) = self.load(candidate.identity()).await? {
            return same_request(candidate, plan)
                .map(|plan| AdmissionState::Committed(Box::new(plan)));
        }
        let mut present = 0;
        for index in 0..candidate.manifest.chunk_hashes.len() {
            if let Some(bytes) = self.read(&candidate.manifest.chunk_id(index)).await? {
                codec::open_chunk(&candidate.manifest, index, &bytes)?;
                present += 1;
            }
        }
        Ok(if present == 0 {
            AdmissionState::Absent
        } else {
            AdmissionState::Partial {
                present,
                total: candidate.manifest.chunk_hashes.len(),
            }
        })
    }

    pub async fn lookup_task(
        &self,
        owner: &str,
        admission_id: &str,
    ) -> Result<Option<AdmittedPlan>, AdmissionError> {
        if owner.is_empty() || admission_id.is_empty() {
            return Err(AdmissionError::Invalid);
        }
        let Some(bytes) = self.read(admission_id).await? else {
            return Ok(None);
        };
        let manifest: Manifest = codec::decode(&bytes)?;
        if manifest.id != admission_id || manifest.identity.owner != owner {
            return Ok(None);
        }
        self.restore(&manifest).await.map(Some)
    }

    pub(super) async fn restore(
        &self,
        manifest: &Manifest,
    ) -> Result<AdmittedPlan, AdmissionError> {
        let mut payload = Vec::with_capacity(manifest.payload_len);
        for index in 0..manifest.chunk_hashes.len() {
            let bytes = trace::admission(Stage::ChunkRead, self.read(&manifest.chunk_id(index)))
                .await?
                .ok_or(AdmissionError::MissingChunk)?;
            payload.extend(codec::open_chunk(manifest, index, &bytes)?);
        }
        if payload.len() != manifest.payload_len || codec::hash(&payload) != manifest.payload_hash {
            return Err(AdmissionError::Corrupt);
        }
        let plan: AdmittedPlan = codec::decode(&payload)?;
        plan.validate()?;
        if plan.identity != manifest.identity
            || plan.admission_id != manifest.id
            || plan.request_hash != manifest.request_hash
            || plan.admitted_at != manifest.admitted_at
            || plan.retained_until != manifest.retained_until
        {
            return Err(AdmissionError::Corrupt);
        }
        manifest.validate((self.clock)())?;
        Ok(plan)
    }

    pub(super) async fn read(&self, id: &str) -> Result<Option<Vec<u8>>, AdmissionError> {
        let url = self
            .records
            .record_url(id)
            .map_err(|_| AdmissionError::Corrupt)?;
        let value = match self.records.get(url.as_str()).await {
            Ok(value) => value,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(AdmissionError::Unavailable),
        };
        if value.version != 1 {
            return Err(AdmissionError::Corrupt);
        }
        open_interaction(&self.keys, id, &value.blob)
            .map(|opened| Some(opened.payload))
            .map_err(|_| AdmissionError::Corrupt)
    }

    async fn create_immutable(
        &self,
        id: &str,
        manifest: &Manifest,
        bytes: &[u8],
    ) -> Result<(), AdmissionError> {
        let result = self.create(id, manifest, bytes).await;
        match result {
            Ok(()) => Ok(()),
            Err(RecordError::Conflict { .. } | RecordError::Unavailable(_)) => {
                match self.read(id).await {
                    Ok(Some(existing)) if existing == bytes => Ok(()),
                    Ok(Some(_)) => Err(AdmissionError::Corrupt),
                    Ok(None) if matches!(result, Err(RecordError::Conflict { .. })) => {
                        Err(AdmissionError::ExpiredOrUnavailable)
                    }
                    Ok(None) | Err(AdmissionError::Unavailable) => Err(AdmissionError::Uncertain),
                    Err(error) => Err(error),
                }
            }
            Err(_) => Err(AdmissionError::Unavailable),
        }
    }

    pub(super) async fn create(
        &self,
        id: &str,
        manifest: &Manifest,
        bytes: &[u8],
    ) -> Result<(), RecordError> {
        let blob = seal_interaction(&self.keys, id, bytes)
            .map_err(|_| RecordError::Rejected("routed record encryption failed".into()))?;
        self.records
            .put_expiring(
                &RecordId::new(id)
                    .map_err(|_| RecordError::Rejected("invalid routed ID".into()))?,
                &manifest.identity.owner,
                &blob,
                None,
                Some(manifest.retained_until),
            )
            .await
            .and_then(|version| {
                if version == 1 {
                    Ok(())
                } else {
                    Err(RecordError::Malformed("invalid create version".into()))
                }
            })
    }

    pub(super) async fn create_guarded(
        &self,
        id: &str,
        manifest: &Manifest,
        bytes: &[u8],
        channel_record_id: &str,
        channel_version: u64,
        authority_guards: &[AuthorityGuard],
    ) -> Result<(), RecordError> {
        let blob = seal_interaction(&self.keys, id, bytes)
            .map_err(|_| RecordError::Rejected("routed record encryption failed".into()))?;
        let version = self
            .records
            .put_interaction_guarded(
                id,
                &manifest.identity.owner,
                &blob,
                manifest.retained_until,
                channel_record_id,
                channel_version,
                authority_guards,
            )
            .await?;
        if version == 1 {
            Ok(())
        } else {
            Err(RecordError::Malformed("invalid create version".into()))
        }
    }
}

fn same_request(candidate: &Candidate, plan: AdmittedPlan) -> Result<AdmittedPlan, AdmissionError> {
    if plan.is_compatibility() {
        let incoming: AdmittedPlan = codec::decode(&candidate.bytes)?;
        let [target] = incoming.targets.as_slice() else {
            return Err(AdmissionError::Conflict);
        };
        let retained = &plan.targets[0].launch;
        if retained.idempotency_key != target.launch.idempotency_key
            || retained.event.consumer != target.launch.event.consumer
            || retained.event.conversation_id != target.launch.event.conversation_id
            || retained.event.partition_key != target.launch.event.partition_key
            || retained.event.lineage != target.launch.event.lineage
        {
            return Err(AdmissionError::Conflict);
        }
    }
    if plan.request_hash != candidate.request_hash() {
        Err(AdmissionError::Conflict)
    } else {
        Ok(plan)
    }
}
