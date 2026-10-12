use super::*;
use crate::{
    bots::{BotAdmission, BotProfileResolver, BotReply, CreateBot},
    Caller, Capability,
};
use std::sync::Arc;

pub struct EventWorkerProfileService {
    store: Arc<dyn EventWorkerProfileStore>,
    resolver: Arc<dyn BotProfileResolver>,
    provider: String,
}
impl EventWorkerProfileService {
    pub fn new(
        store: Arc<dyn EventWorkerProfileStore>,
        resolver: Arc<dyn BotProfileResolver>,
        provider: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            resolver,
            provider,
        })
    }
    pub async fn get(
        &self,
        caller: &Caller,
        id: &str,
        revision: Option<u32>,
    ) -> Result<EventWorkerProfile, BotError> {
        require(caller)?;
        self.owned(&caller.subject, id, revision).await
    }
    async fn owned(
        &self,
        owner: &SubjectId,
        id: &str,
        revision: Option<u32>,
    ) -> Result<EventWorkerProfile, BotError> {
        validate_id(id)?;
        let row = self
            .store
            .get(id, revision)
            .await?
            .filter(|p| &p.owner == owner)
            .ok_or(BotError::NotFound)?;
        row.validate()?;
        Ok(row)
    }
    pub async fn list(
        &self,
        caller: &Caller,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventWorkerProfile>, BotError> {
        require(caller)?;
        if !(1..=200).contains(&limit) {
            return Err(BotError::Invalid("limit must be between 1 and 200".into()));
        }
        if let Some(id) = after {
            validate_id(id)?;
        }
        self.store.list(&caller.subject, after, limit).await
    }
    pub async fn create(
        &self,
        caller: &Caller,
        key: &str,
        input: EventWorkerProfileInput,
    ) -> Result<EventWorkerProfile, BotError> {
        require(caller)?;
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(BotError::Invalid("Idempotency-Key is required".into()));
        }
        let hash = digest(&("event-worker-profile-create-v1", &caller.subject, key))?;
        let id = format!("ewp_{}", &hash[..32]);
        let fingerprint = digest(&input)?;
        if let Some(row) = self.store.get(&id, Some(1)).await? {
            return replay(row, &caller.subject, &fingerprint);
        }
        let now = crate::now_rfc3339();
        let row = self
            .prepare(&caller.subject, id, 1, input, now, fingerprint.clone())
            .await?;
        match self.store.save(None, &row).await {
            Ok(()) => Ok(row),
            Err(BotError::Conflict) => replay(
                self.store
                    .get(&row.id, Some(1))
                    .await?
                    .ok_or(BotError::Conflict)?,
                &caller.subject,
                &fingerprint,
            ),
            Err(e) => Err(e),
        }
    }
    pub async fn update(
        &self,
        caller: &Caller,
        id: &str,
        expected: u32,
        input: EventWorkerProfileInput,
    ) -> Result<EventWorkerProfile, BotError> {
        let old = self.get(caller, id, None).await?;
        if old.retired_at.is_some() {
            return Err(BotError::NotFound);
        }
        if old.revision != expected {
            if old.revision == expected.saturating_add(1) && old.source == input {
                return Ok(old);
            }
            return Err(BotError::Conflict);
        }
        let row = self
            .prepare(
                &caller.subject,
                old.id,
                expected.checked_add(1).ok_or(BotError::Conflict)?,
                input,
                old.created_at,
                old.creation_fingerprint,
            )
            .await?;
        self.store.save(Some(expected), &row).await?;
        Ok(row)
    }
    pub async fn retire(
        &self,
        caller: &Caller,
        id: &str,
        expected: u32,
    ) -> Result<EventWorkerProfile, BotError> {
        let mut row = self.get(caller, id, None).await?;
        if row.retired_at.is_some() && row.revision == expected.saturating_add(1) {
            return Ok(row);
        }
        if row.revision != expected || row.retired_at.is_some() {
            return Err(BotError::Conflict);
        }
        row.revision = row.revision.checked_add(1).ok_or(BotError::Conflict)?;
        row.updated_at = crate::now_rfc3339();
        row.retired_at = Some(row.updated_at.clone());
        self.store.save(Some(expected), &row).await?;
        Ok(row)
    }
    async fn prepare(
        &self,
        owner: &SubjectId,
        id: String,
        revision: u32,
        source: EventWorkerProfileInput,
        created_at: String,
        creation_fingerprint: String,
    ) -> Result<EventWorkerProfile, BotError> {
        if source.name.trim().is_empty()
            || source.name.len() > 80
            || source.name.chars().any(char::is_control)
            || source.description.len() > 2000
        {
            return Err(BotError::Invalid(
                "invalid profile name or description".into(),
            ));
        }
        let request = CreateBot {
            name: source.name.clone(),
            external_bot_id: "profile-authoring".into(),
            connection_ref: None,
            profile: source.agent_profile.clone(),
            sandbox_profile: Some(source.sandbox_profile.clone()),
            workspace_profile: Some(source.workspace_profile.clone()),
            event_worker_profile: None,
            admission: BotAdmission::All,
            reply: BotReply {
                auto_final: true,
                ack_after_ms: 2000,
            },
        };
        request.validate()?;
        let profiles = self.resolver.resolve(owner, &request).await?;
        let mut snapshot = projection::snapshot(&projection::WorkerProjection {
            name: &source.name,
            owner,
            provider: &self.provider,
            profiles: &profiles,
        })?;
        snapshot["metadata"]["description"] = Value::String(source.description.clone());
        let row = EventWorkerProfile {
            provider: self.provider.clone(),
            id,
            owner: owner.clone(),
            revision,
            source,
            profiles,
            digest: format!("sha256:{}", digest(&snapshot)?),
            snapshot,
            created_at,
            updated_at: crate::now_rfc3339(),
            retired_at: None,
            creation_fingerprint,
        };
        row.validate()?;
        Ok(row)
    }
}
#[async_trait::async_trait]
impl BotProfileResolver for EventWorkerProfileService {
    async fn resolve(
        &self,
        owner: &SubjectId,
        request: &CreateBot,
    ) -> Result<BotProfiles, BotError> {
        let Some(reference) = &request.event_worker_profile else {
            return self.resolver.resolve(owner, request).await;
        };
        let current = self.owned(owner, &reference.id, None).await?;
        if current.retired_at.is_some() {
            return Err(BotError::NotFound);
        }
        let row = self
            .owned(owner, &reference.id, Some(reference.revision))
            .await?;
        if row.source.agent_profile != request.profile
            || Some(&row.source.sandbox_profile) != request.sandbox_profile.as_ref()
            || Some(&row.source.workspace_profile) != request.workspace_profile.as_ref()
        {
            return Err(BotError::Invalid(
                "profile references must match the Event Worker Profile revision".into(),
            ));
        }
        let pin = row.pin();
        let mut profiles = row.profiles;
        profiles.event_worker_profile = Some(pin);
        Ok(profiles)
    }
}
fn require(caller: &Caller) -> Result<(), BotError> {
    caller
        .require(Capability::ManageOwnProfiles)
        .map_err(|_| BotError::Forbidden)
}
fn digest(value: &impl Serialize) -> Result<String, BotError> {
    crate::profiles::content_digest(value).map_err(|_| BotError::Unavailable)
}
fn replay(
    row: EventWorkerProfile,
    owner: &SubjectId,
    fingerprint: &str,
) -> Result<EventWorkerProfile, BotError> {
    if row.owner != *owner || row.creation_fingerprint != fingerprint {
        Err(BotError::Conflict)
    } else {
        row.validate()?;
        Ok(row)
    }
}
