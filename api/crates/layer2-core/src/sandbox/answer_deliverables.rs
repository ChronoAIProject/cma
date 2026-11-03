mod runtime;
mod types;
pub use types::*;

use super::agent_filesystem::{AgentFileError, AgentFileService};
use super::{agent_artifacts::AgentArtifactService, AgentReadError, AgentReadService};
use crate::document_artifacts::{DocumentError, DocumentService};
use crate::resource_access::{CallerContext, ResourceAccessError};
use crate::{AgentId, Caller};
use std::sync::Arc;

pub struct DeliverableAccess<'a> {
    pub caller: &'a Caller,
    pub context: Option<&'a CallerContext>,
    pub credential: Option<&'a str>,
    pub document_token: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum DeliverableError {
    #[error(transparent)]
    Answer(#[from] AgentReadError),
    #[error(transparent)]
    Artifact(#[from] ResourceAccessError),
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error(transparent)]
    File(#[from] AgentFileError),
    #[error("This resource capability is unavailable")]
    Unavailable,
    #[error("Invalid deliverable reference")]
    Invalid,
    #[error("Deliverable is not part of this final answer")]
    NotFound,
    #[error("Deliverable bytes differ from the referenced version")]
    Changed,
}

pub struct AnswerDeliverableService {
    reads: Arc<AgentReadService>,
    artifacts: Arc<AgentArtifactService>,
    documents: Option<Arc<DocumentService>>,
    files: Option<Arc<AgentFileService>>,
}

impl AnswerDeliverableService {
    pub fn new(reads: Arc<AgentReadService>, artifacts: Arc<AgentArtifactService>) -> Arc<Self> {
        Arc::new(Self {
            reads,
            artifacts,
            documents: None,
            files: None,
        })
    }

    pub fn with_documents(self: &Arc<Self>, documents: Arc<DocumentService>) -> Arc<Self> {
        Arc::new(Self {
            reads: self.reads.clone(),
            artifacts: self.artifacts.clone(),
            documents: Some(documents),
            files: self.files.clone(),
        })
    }

    pub fn with_files(self: &Arc<Self>, files: Arc<AgentFileService>) -> Arc<Self> {
        Arc::new(Self {
            reads: self.reads.clone(),
            artifacts: self.artifacts.clone(),
            documents: self.documents.clone(),
            files: Some(files),
        })
    }

    pub async fn describe(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        resource: DeliverableResource,
    ) -> Result<DeliverableDescription, DeliverableError> {
        self.reads.read(access.caller, agent).await?;
        let reference = match &resource {
            DeliverableResource::LiveFile { path } => {
                let receipt = self
                    .file(access, agent, path, layer3_adapter::MAX_AGENT_FILE_BYTES)
                    .await?;
                DeliverableReference {
                    agent_id: agent.clone(),
                    name: types::file_name(path.rsplit('/').next().unwrap_or("deliverable")),
                    resource,
                    media_type: "application/octet-stream".into(),
                    size_bytes: receipt.size_bytes,
                    sha256: receipt.sha256,
                }
            }
            DeliverableResource::AgentArtifact { id } => {
                let (metadata, _) = self
                    .artifacts
                    .read(access.caller, access.context, access.credential, agent, id)
                    .await?;
                DeliverableReference {
                    agent_id: agent.clone(),
                    resource,
                    name: metadata.name,
                    media_type: metadata.media_type,
                    size_bytes: metadata.size_bytes,
                    sha256: metadata.sha256,
                }
            }
            DeliverableResource::Document { id, revision } => {
                if *revision == 0 {
                    return Err(DeliverableError::Invalid);
                }
                let documents = self
                    .documents
                    .as_ref()
                    .ok_or(DeliverableError::Unavailable)?;
                let metadata = documents
                    .revisions(access.caller, id, access.document_token, revision - 1, 1)
                    .await?
                    .items
                    .into_iter()
                    .find(|item| item.revision == *revision)
                    .ok_or(DeliverableError::NotFound)?;
                DeliverableReference {
                    agent_id: agent.clone(),
                    resource,
                    name: format!("{}.md", types::file_name(&metadata.title)),
                    media_type: "text/markdown".into(),
                    size_bytes: metadata.size_bytes,
                    sha256: metadata.sha256,
                }
            }
        };
        self.bytes(access, agent, &reference).await?;
        Ok(DeliverableDescription {
            marker: reference.marker()?,
            reference,
        })
    }

    pub async fn list(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        answer: u64,
    ) -> Result<AnswerDeliverables, DeliverableError> {
        let (reading, references) = self.references(access, agent, answer).await?;
        let turn_sequence = reading.turn_sequence;
        let mut items = Vec::new();
        for reference in references {
            let status = status(self.metadata(access, agent, &reference).await);
            items.push(AnswerDeliverable {
                id: reference.id()?,
                reference,
                status,
            });
        }
        self.reads.validate_final_answer(&reading).await?;
        Ok(AnswerDeliverables {
            agent_id: agent.clone(),
            answer_id: format!("event:{answer}"),
            turn_sequence,
            items,
        })
    }

    pub async fn inspect(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        answer: u64,
        delivery: &str,
    ) -> Result<AnswerDeliverable, DeliverableError> {
        Ok(self
            .read_item(access, agent, answer, delivery, true)
            .await?
            .item)
    }

    pub async fn read_item(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        answer: u64,
        delivery: &str,
        verify: bool,
    ) -> Result<AnswerDeliverableReading, DeliverableError> {
        let (reading, references) = self.references(access, agent, answer).await?;
        let reference = references
            .into_iter()
            .find(|reference| reference.id().is_ok_and(|id| id == delivery))
            .ok_or(DeliverableError::NotFound)?;
        let status = status(if verify {
            self.bytes(access, agent, &reference)
                .await
                .map(|(_, status)| status)
        } else {
            self.metadata(access, agent, &reference).await
        });
        self.reads.validate_final_answer(&reading).await?;
        Ok(AnswerDeliverableReading {
            agent_id: agent.clone(),
            answer_id: format!("event:{answer}"),
            turn_sequence: reading.turn_sequence,
            item: AnswerDeliverable {
                id: reference.id()?,
                reference,
                status,
            },
        })
    }

    async fn metadata(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        reference: &DeliverableReference,
    ) -> Result<DeliverableStatus, DeliverableError> {
        let (name, media_type, size_bytes, sha256, status) = match &reference.resource {
            DeliverableResource::LiveFile { path } => {
                self.files
                    .as_ref()
                    .ok_or(DeliverableError::Unavailable)?
                    .check_read(
                        access.caller,
                        access.context,
                        access.credential,
                        agent,
                        path,
                    )
                    .await?;
                return Ok(DeliverableStatus::Unchecked);
            }
            DeliverableResource::AgentArtifact { id } => {
                let metadata = self
                    .artifacts
                    .metadata(access.caller, access.context, access.credential, agent, id)
                    .await?;
                (
                    metadata.name,
                    metadata.media_type,
                    metadata.size_bytes,
                    metadata.sha256,
                    DeliverableStatus::Ready,
                )
            }
            DeliverableResource::Document { id, revision } => {
                let documents = self
                    .documents
                    .as_ref()
                    .ok_or(DeliverableError::Unavailable)?;
                let metadata = documents
                    .revisions(
                        access.caller,
                        id,
                        access.document_token,
                        revision.checked_sub(1).ok_or(DeliverableError::Invalid)?,
                        1,
                    )
                    .await?
                    .items
                    .into_iter()
                    .find(|item| item.revision == *revision)
                    .ok_or(DeliverableError::NotFound)?;
                let current = documents
                    .read(access.caller, id, access.document_token)
                    .await?;
                let status = if current.source_revision == *revision {
                    DeliverableStatus::Ready
                } else {
                    DeliverableStatus::VersionChanged
                };
                (
                    format!("{}.md", types::file_name(&metadata.title)),
                    "text/markdown".into(),
                    metadata.size_bytes,
                    metadata.sha256,
                    status,
                )
            }
        };
        if name != reference.name
            || media_type != reference.media_type
            || size_bytes != reference.size_bytes
            || sha256 != reference.sha256
        {
            return Err(DeliverableError::Changed);
        }
        Ok(status)
    }

    pub async fn download(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        answer: u64,
        delivery: &str,
    ) -> Result<(DeliverableReference, Vec<u8>), DeliverableError> {
        let (reading, references) = self.references(access, agent, answer).await?;
        let reference = references
            .into_iter()
            .find(|reference| reference.id().is_ok_and(|id| id == delivery))
            .ok_or(DeliverableError::NotFound)?;
        let (bytes, _) = self.bytes(access, agent, &reference).await?;
        self.reads.validate_final_answer(&reading).await?;
        Ok((reference, bytes))
    }

    async fn references(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        answer: u64,
    ) -> Result<
        (
            super::agent_reads::AgentFinalAnswer,
            Vec<DeliverableReference>,
        ),
        DeliverableError,
    > {
        let reading = self
            .reads
            .final_answer(
                access.caller,
                access.context,
                access.credential,
                agent,
                answer,
            )
            .await?;
        let references = types::references(&reading.message.content, agent)?;
        Ok((reading, references))
    }

    async fn bytes(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        reference: &DeliverableReference,
    ) -> Result<(Vec<u8>, DeliverableStatus), DeliverableError> {
        let (bytes, status) = match &reference.resource {
            DeliverableResource::LiveFile { path } => {
                let receipt = self
                    .file(access, agent, path, reference.size_bytes.max(1))
                    .await
                    .map_err(|error| match error {
                        DeliverableError::File(AgentFileError::TooLarge) => {
                            DeliverableError::Changed
                        }
                        error => error,
                    })?;
                (
                    receipt.content().map_err(AgentFileError::from)?,
                    DeliverableStatus::Ready,
                )
            }
            DeliverableResource::AgentArtifact { id } => {
                let (_, bytes) = self
                    .artifacts
                    .read(access.caller, access.context, access.credential, agent, id)
                    .await?;
                (bytes, DeliverableStatus::Ready)
            }
            DeliverableResource::Document { id, revision } => {
                let documents = self
                    .documents
                    .as_ref()
                    .ok_or(DeliverableError::Unavailable)?;
                let source = documents
                    .source(access.caller, id, access.document_token, Some(*revision))
                    .await?;
                let current = documents
                    .read(access.caller, id, access.document_token)
                    .await?;
                let status = if current.source_revision == *revision {
                    DeliverableStatus::Ready
                } else {
                    DeliverableStatus::VersionChanged
                };
                (source.markdown.into_bytes(), status)
            }
        };
        if bytes.len() as u64 != reference.size_bytes || types::digest(&bytes) != reference.sha256 {
            return Err(DeliverableError::Changed);
        }
        Ok((bytes, status))
    }

    async fn file(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        path: &str,
        max_bytes: u64,
    ) -> Result<layer3_adapter::AgentFileReadReceipt, DeliverableError> {
        let files = self.files.as_ref().ok_or(DeliverableError::Unavailable)?;
        let receipt = files
            .read(
                access.caller,
                access.context,
                access.credential,
                agent,
                path,
                max_bytes,
            )
            .await?;
        if receipt.path != path {
            return Err(DeliverableError::Invalid);
        }
        receipt.content().map_err(AgentFileError::from)?;
        Ok(receipt)
    }
}

fn status(result: Result<DeliverableStatus, DeliverableError>) -> DeliverableStatus {
    match result {
        Ok(status) => status,
        Err(DeliverableError::Changed) => DeliverableStatus::Changed,
        Err(DeliverableError::File(AgentFileError::NotFound)) => DeliverableStatus::Missing,
        Err(DeliverableError::File(AgentFileError::TooLarge)) => DeliverableStatus::TooLarge,
        Err(DeliverableError::File(AgentFileError::NotReady)) => DeliverableStatus::NotReady,
        Err(DeliverableError::File(AgentFileError::Access(ResourceAccessError::Access(
            crate::resource_access::AccessError::NotFound
            | crate::resource_access::AccessError::Forbidden,
        )))) => DeliverableStatus::Denied,
        Err(DeliverableError::Document(DocumentError::NotFound)) => DeliverableStatus::Denied,
        Err(DeliverableError::Artifact(ResourceAccessError::Access(
            crate::resource_access::AccessError::NotFound
            | crate::resource_access::AccessError::Forbidden,
        ))) => DeliverableStatus::Denied,
        Err(_) => DeliverableStatus::Unavailable,
    }
}
