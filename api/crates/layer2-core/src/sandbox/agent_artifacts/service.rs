use std::{sync::Arc, time::SystemTime};

use super::*;
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessStoreError, ResourceActor, ResourceCapability,
    ResourceTarget,
};
use crate::{AgentId, Caller};

pub struct AgentArtifactService {
    store: Arc<dyn AgentArtifactStore>,
    access: Arc<ResourceAccessService>,
}
impl AgentArtifactService {
    pub async fn metadata(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
        artifact_id: &str,
    ) -> Result<AgentArtifactMetadata, ResourceAccessError> {
        tokio::time::timeout(ArtifactPublication::LIFETIME, async {
            let authorized = self.authorize(caller, context, credential, id).await?;
            if !valid_artifact_id(artifact_id) {
                return Err(ResourceAccessStoreError::NotFound.into());
            }
            let binding = ArtifactBinding::from_snapshot(&authorized.snapshot)?;
            let row = self
                .store
                .agent_artifact_metadata(&binding, artifact_id)
                .await?
                .ok_or(ResourceAccessStoreError::NotFound)?;
            row.validate()?;
            if row.binding != binding || row.metadata.id != artifact_id {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            self.finish(&authorized, id).await?;
            Ok(row.metadata)
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }

    pub fn new(
        store: Arc<dyn AgentArtifactStore>,
        access: Arc<ResourceAccessService>,
    ) -> Arc<Self> {
        Arc::new(Self { store, access })
    }

    /// CallerContext enables organization owners; legacy callers remain private
    /// personal owners. The body supplies no owner, actor, path or source URL.
    pub async fn publish(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        key: &str,
        body: PublishAgentArtifact,
    ) -> Result<AgentArtifactMetadata, ResourceAccessError> {
        let started_at = SystemTime::now();
        let deadline = tokio::time::Instant::now() + ArtifactPublication::LIFETIME;
        if key.is_empty() || key.len() > 255 || key.bytes().any(|b| b.is_ascii_control()) {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        let bytes = body.content()?;
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let (authorized, actor) = if let Some(context) = context {
            if caller != &context.caller
                || !context.validate_principal()
                || !matches!(&context.actor, ResourceActor::Human { subject }
                    | ResourceActor::Api { subject, delegation_id: None } if subject == &caller.subject)
            {
                return Err(AccessError::NotFound.into());
            }
            (
                self.access
                    .authorize(
                        context,
                        credential,
                        &target,
                        ResourceCapability::ManageSharing,
                        AccessPurpose::Mutation { started_at },
                    )
                    .await?,
                context.actor.clone(),
            )
        } else {
            (
                self.access
                    .authorize_personal(caller, &target, ResourceCapability::ManageSharing)
                    .await?,
                ResourceActor::Human {
                    subject: caller.subject.clone(),
                },
            )
        };
        let binding = ArtifactBinding::from_snapshot(&authorized.snapshot)?;
        let authorization =
            authorized.authorization_audit(&actor, &target, ResourceCapability::ManageSharing)?;
        let id = format!(
            "art_{}",
            super::types::digest(
                &serde_json::to_vec(&("agent-artifact-v1", &binding, key,))
                    .map_err(|_| ResourceAccessStoreError::Invalid)?
            )
        );
        let record = StoredAgentArtifact {
            metadata: AgentArtifactMetadata {
                id,
                agent_id: agent_id.clone(),
                name: body.name,
                media_type: body.media_type,
                size_bytes: bytes.len() as u64,
                sha256: super::types::digest(&bytes),
                created_at: authorization.captured_at.clone(),
            },
            binding,
            authorization,
            content_base64: body.content_base64,
        };
        record.content()?;
        let publication = ArtifactPublication {
            authorized,
            record,
            deadline,
        };
        let value = tokio::time::timeout_at(deadline, self.store.publish_artifact(&publication))
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        Ok(value.metadata)
    }

    pub async fn list(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
        page: &ArtifactPageRequest,
    ) -> Result<super::super::store::Page<AgentArtifactMetadata>, ResourceAccessError> {
        tokio::time::timeout(ArtifactPublication::LIFETIME, async {
            let authorized = self.authorize(caller, context, credential, id).await?;
            page.validate()?;
            let binding = ArtifactBinding::from_snapshot(&authorized.snapshot)?;
            let mut rows = self.store.agent_artifacts(&binding, page).await?;
            if rows.len() > usize::from(page.limit) + 1 {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            let mut previous = page.cursor.as_deref();
            for row in &rows {
                row.content()?;
                if row.binding != binding
                    || previous.is_some_and(|id| row.metadata.id.as_str() <= id)
                {
                    return Err(ResourceAccessStoreError::Corrupt.into());
                }
                previous = Some(&row.metadata.id);
            }
            let more = rows.len() > usize::from(page.limit);
            rows.truncate(usize::from(page.limit));
            let next = more.then(|| {
                rows.last()
                    .expect("positive page limit")
                    .metadata
                    .id
                    .clone()
            });
            self.finish(&authorized, id).await?;
            Ok(super::super::store::Page {
                items: rows.into_iter().map(|row| row.metadata).collect(),
                next,
            })
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }

    /// Returns buffered verified bytes only after the original authority and
    /// current grant/ancestry fence pass. No reusable signed download URL exists.
    pub async fn read(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
        artifact_id: &str,
    ) -> Result<(AgentArtifactMetadata, Vec<u8>), ResourceAccessError> {
        tokio::time::timeout(ArtifactPublication::LIFETIME, async {
            let authorized = self.authorize(caller, context, credential, id).await?;
            if !valid_artifact_id(artifact_id) {
                return Err(ResourceAccessStoreError::NotFound.into());
            }
            let binding = ArtifactBinding::from_snapshot(&authorized.snapshot)?;
            let row = self
                .store
                .agent_artifact(&binding, artifact_id)
                .await?
                .ok_or(ResourceAccessStoreError::NotFound)?;
            if row.binding != binding || row.metadata.id != artifact_id {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            let bytes = row.content()?;
            self.finish(&authorized, id).await?;
            Ok((row.metadata, bytes))
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }

    async fn authorize(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AuthorizedResource, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        if let Some(context) = context {
            if caller != &context.caller {
                return Err(AccessError::NotFound.into());
            }
            self.access
                .authorize(
                    context,
                    credential,
                    &target,
                    ResourceCapability::ViewArtifacts,
                    AccessPurpose::Read,
                )
                .await
        } else {
            self.access
                .authorize_personal(caller, &target, ResourceCapability::ViewArtifacts)
                .await
        }
    }
    async fn finish(
        &self,
        authorized: &AuthorizedResource,
        id: &AgentId,
    ) -> Result<(), ResourceAccessError> {
        let current = self
            .access
            .current_read_snapshot(
                authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                ResourceCapability::ViewArtifacts,
            )
            .await?;
        if ArtifactBinding::from_snapshot(&current)?
            != ArtifactBinding::from_snapshot(&authorized.snapshot)?
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }
}
