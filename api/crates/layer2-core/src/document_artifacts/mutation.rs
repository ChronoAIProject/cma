use super::{service::*, *};
use crate::Caller;
use serde::Serialize;

#[derive(Serialize)]
pub(super) enum Command {
    Update(UpdateDocument),
    Grant(String, GrantDocument),
    Revoke(String, u64),
    Link(CreateDocumentLink),
    RevokeLink(String, u64),
    Comment(CreateDocumentComment),
    RemoveComment(String, u64),
}
impl Command {
    fn expected(&self) -> Option<u64> {
        match self {
            Self::Update(v) => Some(v.expected_revision),
            Self::Grant(_, v) => Some(v.expected_revision),
            Self::Link(v) => Some(v.expected_revision),
            Self::Revoke(_, v) | Self::RevokeLink(_, v) | Self::RemoveComment(_, v) => Some(*v),
            Self::Comment(_) => None,
        }
    }
    fn owner_only(&self) -> bool {
        !matches!(self, Self::Comment(_) | Self::RemoveComment(_, _))
    }
    fn revokes_access(&self) -> bool {
        matches!(self, Self::Revoke(..) | Self::RevokeLink(..))
    }
    fn operation(&self) -> &'static str {
        match self {
            Self::Update(_) => "update",
            Self::Grant(..) => "grant",
            Self::Revoke(..) => "revoke",
            Self::Link(_) => "link",
            Self::RevokeLink(..) => "revoke_link",
            Self::Comment(_) => "comment",
            Self::RemoveComment(..) => "remove_comment",
        }
    }
}
impl DocumentService {
    pub async fn update(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        body: UpdateDocument,
    ) -> Result<DocumentView, DocumentError> {
        if body.title.as_ref().is_some_and(|v| !types::valid_title(v))
            || body
                .markdown
                .as_ref()
                .is_some_and(|v| !types::valid_markdown(v))
            || (body.title.is_none() && body.markdown.is_none())
        {
            return Err(DocumentError::Invalid);
        }
        Ok(self
            .mutate(caller, id, None, key, Command::Update(body))
            .await?
            .document)
    }
    pub async fn grant(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        subject: &str,
        body: GrantDocument,
    ) -> Result<DocumentView, DocumentError> {
        Ok(self
            .mutate(caller, id, None, key, Command::Grant(subject.into(), body))
            .await?
            .document)
    }
    pub async fn revoke(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        subject: &str,
        expected_revision: u64,
    ) -> Result<DocumentView, DocumentError> {
        if !types::valid_subject(subject) {
            return Err(DocumentError::Invalid);
        }
        Ok(self
            .mutate(
                caller,
                id,
                None,
                key,
                Command::Revoke(subject.into(), expected_revision),
            )
            .await?
            .document)
    }
    pub async fn create_link(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        body: CreateDocumentLink,
    ) -> Result<DocumentMutation, DocumentError> {
        if !valid_token(&body.token) {
            return Err(DocumentError::Invalid);
        }
        self.mutate(caller, id, None, key, Command::Link(body))
            .await
    }
    pub async fn revoke_link(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        link: &str,
        expected_revision: u64,
    ) -> Result<DocumentView, DocumentError> {
        if !valid_id(link, "dln_") {
            return Err(DocumentError::Invalid);
        }
        Ok(self
            .mutate(
                caller,
                id,
                None,
                key,
                Command::RevokeLink(link.into(), expected_revision),
            )
            .await?
            .document)
    }
    pub async fn comment(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        key: &str,
        body: CreateDocumentComment,
    ) -> Result<DocumentMutation, DocumentError> {
        if body.body.trim().is_empty() || body.body.len() > 8192 || body.body.contains('\0') {
            return Err(DocumentError::Invalid);
        }
        self.mutate(caller, id, token, key, Command::Comment(body))
            .await
    }
    pub async fn remove_comment(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        key: &str,
        comment: &str,
        expected_revision: u64,
    ) -> Result<DocumentView, DocumentError> {
        if !valid_id(comment, "dcm_") {
            return Err(DocumentError::Invalid);
        }
        Ok(self
            .mutate(
                caller,
                id,
                token,
                key,
                Command::RemoveComment(comment.into(), expected_revision),
            )
            .await?
            .document)
    }
    async fn mutate(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        key: &str,
        command: Command,
    ) -> Result<DocumentMutation, DocumentError> {
        self.mutate_guarded(caller, id, token, key, command, None)
            .await
    }
    pub(super) async fn mutate_guarded(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        key: &str,
        command: Command,
        guard: Option<&super::runtime::RuntimeGuard<'_>>,
    ) -> Result<DocumentMutation, DocumentError> {
        principal(caller)?;
        operation_key(key)?;
        let receipt_key = types::fingerprint(&(caller.subject.as_str(), command.operation(), key))?;
        let fingerprint = types::fingerprint(&command)?;
        for _ in 0..4 {
            let mut record = if command.owner_only() {
                self.owner(caller, id).await?
            } else {
                self.authorize(caller, id, token, true).await?
            };
            if let Some(guard) = guard {
                guard.validate(Some(&record)).await?;
            }
            if let Some(receipt) = record.receipts.get(&receipt_key) {
                if receipt.fingerprint != fingerprint {
                    return Err(DocumentError::IdempotencyConflict);
                }
                let mut outcome = receipt.outcome.clone();
                outcome.document.access = access(caller, token, &record)?;
                return Ok(outcome);
            }
            if let Command::Grant(subject, _) = &command {
                // A replay never depends on the recipient's current registration.
                self.require_share_recipient(caller, subject).await?;
            }
            let reserve = self.revocation_reserve_enabled && command.revokes_access();
            if !reserve && record.receipts.len() >= MAX_RECEIPTS {
                return Err(DocumentError::Capacity);
            }
            let previous = record.metadata.revision;
            if command
                .expected()
                .is_some_and(|expected| expected != previous)
            {
                return Err(DocumentError::Conflict);
            }
            record.metadata.revision = previous.checked_add(1).ok_or(DocumentError::Capacity)?;
            record.metadata.updated_at = crate::now_rfc3339();
            let (mut outcome, source) = self.apply(caller, key, &command, &mut record).await?;
            outcome.document = view(caller, token, &record)?;
            record.receipts.insert(
                receipt_key.clone(),
                DocumentReceipt {
                    fingerprint: fingerprint.clone(),
                    outcome: outcome.clone(),
                },
            );
            // Ordinary mutations cannot consume the reserved count or bytes.
            // Revocation removes a live grant/link before recording its receipt;
            // it never evicts a receipt that could otherwise replay an old grant.
            self.validate_write(&record, command.revokes_access())?;
            // This single aggregate CAS fences all concurrent revocations and
            // comment admission. Retry reloads policy before producing effects.
            if let Some(guard) = guard {
                guard.validate(Some(&record)).await?;
            }
            let stored = if let Command::Grant(subject, _) = &command {
                self.store
                    .compare_exchange_grant(previous, &record, subject)
                    .await?
            } else if let Some(source) = source {
                self.store
                    .compare_exchange_source(previous, &record, &source, &receipt_key)
                    .await?
            } else {
                self.store.compare_exchange(previous, &record).await?
            };
            if stored {
                return Ok(outcome);
            }
        }
        Err(DocumentError::Conflict)
    }
    async fn apply(
        &self,
        caller: &Caller,
        key: &str,
        command: &Command,
        record: &mut StoredDocument,
    ) -> Result<(DocumentMutation, Option<DocumentSourceWrite>), DocumentError> {
        let mut comment = None;
        let mut link = None;
        let mut source = None;
        match command {
            Command::Update(body) => {
                if record.revisions.len() >= MAX_REVISIONS {
                    return Err(DocumentError::Capacity);
                }
                let last = record.revisions.last().ok_or(DocumentError::Corrupt)?;
                let mut object = last.object_key.clone();
                if let Some(markdown) = &body.markdown {
                    let uploaded = self
                        .upload(
                            &record.metadata.owner_subject,
                            &record.metadata.id,
                            markdown.as_bytes(),
                        )
                        .await?;
                    object = uploaded.source.object_key();
                    record.metadata.sha256 = uploaded.source.sha256.clone();
                    record.metadata.size_bytes = uploaded.source.bytes;
                    source = Some(uploaded);
                }
                if let Some(title) = &body.title {
                    record.metadata.title = title.clone();
                    record.title_search = title.to_lowercase();
                }
                record.metadata.source_revision = record.metadata.revision;
                record.revisions.push(StoredRevision {
                    object_key: object,
                    metadata: DocumentRevision {
                        revision: record.metadata.revision,
                        title: record.metadata.title.clone(),
                        sha256: record.metadata.sha256.clone(),
                        size_bytes: record.metadata.size_bytes,
                        created_at: record.metadata.updated_at.clone(),
                        author_subject: caller.subject.to_string(),
                    },
                });
            }
            Command::Grant(subject, body) => {
                if let Some(share) = record.shares.iter_mut().find(|s| &s.subject == subject) {
                    share.permission = body.permission;
                } else {
                    if record.shares.len() >= MAX_GRANTS {
                        return Err(DocumentError::Capacity);
                    }
                    record.shares.push(DocumentShare {
                        subject: subject.clone(),
                        permission: body.permission,
                    });
                }
            }
            Command::Revoke(subject, _) => {
                if !record.shares.iter().any(|s| &s.subject == subject) {
                    return Err(DocumentError::NotFound);
                }
                record.shares.retain(|s| &s.subject != subject);
            }
            Command::Link(body) => {
                if record.links.len() >= MAX_LINKS {
                    return Err(DocumentError::Capacity);
                }
                let token_sha256 = digest(body.token.as_bytes());
                if record.links.iter().any(|l| l.token_sha256 == token_sha256) {
                    return Err(DocumentError::Conflict);
                }
                let metadata = DocumentLink {
                    id: types::identity("dln_", &[&record.metadata.id, key]),
                    permission: body.permission,
                    created_at: record.metadata.updated_at.clone(),
                };
                link = Some(metadata.clone());
                record.links.push(StoredLink {
                    metadata,
                    token_sha256,
                });
            }
            Command::RevokeLink(id, _) => {
                if !record.links.iter().any(|l| &l.metadata.id == id) {
                    return Err(DocumentError::NotFound);
                }
                record.links.retain(|l| &l.metadata.id != id);
            }
            Command::Comment(body) => {
                if record.comments.len() >= MAX_COMMENTS {
                    return Err(DocumentError::Capacity);
                }
                let revision = body
                    .document_revision
                    .unwrap_or(record.metadata.source_revision);
                if !record
                    .revisions
                    .iter()
                    .any(|r| r.metadata.revision == revision)
                {
                    return Err(DocumentError::Invalid);
                }
                let value = DocumentComment {
                    id: types::identity(
                        "dcm_",
                        &[&record.metadata.id, caller.subject.as_str(), key],
                    ),
                    author_subject: caller.subject.to_string(),
                    body: body.body.clone(),
                    document_revision: revision,
                    created_at: record.metadata.updated_at.clone(),
                };
                comment = Some(value.clone());
                record.comments.push(value);
            }
            Command::RemoveComment(id, _) => {
                let value = record
                    .comments
                    .iter()
                    .find(|c| &c.id == id)
                    .ok_or(DocumentError::NotFound)?;
                if record.metadata.owner_subject != caller.subject.as_str()
                    && value.author_subject != caller.subject.as_str()
                {
                    return Err(DocumentError::NotFound);
                }
                record.comments.retain(|c| &c.id != id);
            }
        }
        Ok((
            DocumentMutation {
                document: record.metadata.clone(),
                comment,
                link,
            },
            source,
        ))
    }
}
