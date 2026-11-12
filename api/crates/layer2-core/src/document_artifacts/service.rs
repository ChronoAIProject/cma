use super::*;
use crate::{Caller, Capability};
use std::sync::Arc;
use std::time::Duration;

pub struct DocumentService {
    pub(super) store: Arc<dyn DocumentStore>,
    pub(super) blobs: Arc<dyn DocumentBlobStore>,
    pub(super) revocation_reserve_enabled: bool,
}
impl DocumentService {
    pub fn new(store: Arc<dyn DocumentStore>, blobs: Arc<dyn DocumentBlobStore>) -> Arc<Self> {
        Self::with_revocation_reserve(store, blobs, false)
    }
    /// Enable only after this compatible reader is the deployed rollback target.
    pub fn with_revocation_reserve(
        store: Arc<dyn DocumentStore>,
        blobs: Arc<dyn DocumentBlobStore>,
        enabled: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            blobs,
            revocation_reserve_enabled: enabled,
        })
    }
    pub async fn create(
        &self,
        caller: &Caller,
        key: &str,
        body: CreateDocument,
    ) -> Result<DocumentView, DocumentError> {
        self.create_guarded(caller, key, body, None).await
    }
    pub(super) async fn create_guarded(
        &self,
        caller: &Caller,
        key: &str,
        body: CreateDocument,
        guard: Option<&super::runtime::RuntimeGuard<'_>>,
    ) -> Result<DocumentView, DocumentError> {
        principal(caller)?;
        operation_key(key)?;
        if !types::valid_title(&body.title)
            || !types::valid_markdown(&body.markdown)
            || body
                .origin_agent_id
                .as_ref()
                .is_some_and(|id| !types::valid_id(id, "agt_"))
        {
            return Err(DocumentError::Invalid);
        }
        let id = types::identity("doc_", &[caller.subject.as_str(), key]);
        let fingerprint = types::fingerprint(&body)?;
        if let Some(existing) = self.store.get(&id).await? {
            if let Some(guard) = guard {
                guard.validate(Some(&existing)).await?;
            }
            return creation_replay(caller, existing, &fingerprint);
        }
        let now = crate::now_rfc3339();
        let sha256 = digest(body.markdown.as_bytes());
        let object_key = object_key(caller.subject.as_str(), &id, &sha256);
        self.upload(&object_key, body.markdown.as_bytes().to_vec())
            .await?;
        let metadata = DocumentView {
            id,
            owner_subject: caller.subject.to_string(),
            title: body.title.clone(),
            revision: 1,
            source_revision: 1,
            sha256,
            size_bytes: body.markdown.len() as u64,
            created_at: now.clone(),
            updated_at: now.clone(),
            origin_agent_id: body.origin_agent_id,
            access: DocumentAccess::Owner,
        };
        let record = StoredDocument {
            runtime_author_agent_id: guard.and_then(|_| metadata.origin_agent_id.clone()),
            title_search: metadata.title.to_lowercase(),
            revisions: vec![StoredRevision {
                metadata: DocumentRevision {
                    revision: 1,
                    title: body.title,
                    sha256: metadata.sha256.clone(),
                    size_bytes: metadata.size_bytes,
                    created_at: now,
                    author_subject: caller.subject.to_string(),
                },
                object_key,
            }],
            metadata: metadata.clone(),
            shares: vec![],
            links: vec![],
            comments: vec![],
            receipts: Default::default(),
            creation_fingerprint: fingerprint.clone(),
        };
        if let Some(guard) = guard {
            guard.validate(Some(&record)).await?;
        }
        self.validate_write(&record, false)?;
        if self.store.insert(&record).await? {
            return Ok(metadata);
        }
        creation_replay(
            caller,
            self.store
                .get(&record.metadata.id)
                .await?
                .ok_or(DocumentError::Unavailable)?,
            &fingerprint,
        )
    }
    pub async fn read(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
    ) -> Result<DocumentView, DocumentError> {
        let record = self.authorize(caller, id, token, false).await?;
        view(caller, token, &record)
    }
    pub async fn list(
        &self,
        caller: &Caller,
        query: &DocumentQuery,
    ) -> Result<DocumentPage<DocumentView>, DocumentError> {
        principal(caller)?;
        query.validate()?;
        let mut records = self.store.list(caller.subject.as_str(), query).await?;
        if records.len() > query.limit() + 1 {
            return Err(DocumentError::Corrupt);
        }
        let more = records.len() > query.limit();
        records.truncate(query.limit());
        let next = if more {
            records.last().map(|r| r.metadata.id.clone())
        } else {
            None
        };
        let mut items = Vec::new();
        for row in records {
            // Inventory metadata and grants come from one atomic majority read.
            // Source retrieval independently checks current policy before delivery.
            match row.view(caller.subject.as_str()) {
                Ok(item) => items.push(item),
                Err(DocumentError::NotFound) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(DocumentPage { items, next })
    }
    pub async fn source(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        revision: Option<u64>,
    ) -> Result<DocumentSource, DocumentError> {
        let record = self.authorize(caller, id, token, false).await?;
        let revision = revision.unwrap_or(record.metadata.source_revision);
        let source = record
            .revisions
            .iter()
            .find(|r| r.metadata.revision == revision)
            .ok_or(DocumentError::NotFound)?;
        let bytes =
            tokio::time::timeout(Duration::from_secs(15), self.blobs.get(&source.object_key))
                .await
                .map_err(|_| DocumentError::Unavailable)??;
        if bytes.len() > MAX_DOCUMENT_BYTES
            || bytes.len() as u64 != source.metadata.size_bytes
            || digest(&bytes) != source.metadata.sha256
        {
            return Err(DocumentError::Corrupt);
        }
        let markdown = String::from_utf8(bytes).map_err(|_| DocumentError::Corrupt)?;
        // Buffer before this final authorization check. No long-lived signed URL
        // or stream can retain a grant that was revoked during object retrieval.
        self.authorize(caller, id, token, false).await?;
        Ok(DocumentSource {
            id: id.to_owned(),
            revision,
            markdown,
        })
    }
    pub async fn revisions(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        after: u64,
        limit: u16,
    ) -> Result<DocumentPage<DocumentRevision>, DocumentError> {
        page_limit(limit)?;
        let record = self.authorize(caller, id, token, false).await?;
        let mut items: Vec<_> = record
            .revisions
            .into_iter()
            .filter(|r| r.metadata.revision > after)
            .map(|r| r.metadata)
            .take(usize::from(limit) + 1)
            .collect();
        let more = items.len() > usize::from(limit);
        items.truncate(usize::from(limit));
        let next = if more {
            items.last().map(|r| r.revision.to_string())
        } else {
            None
        };
        self.authorize(caller, id, token, false).await?;
        Ok(DocumentPage { items, next })
    }
    pub async fn comments(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        after: Option<&str>,
        limit: u16,
    ) -> Result<DocumentPage<DocumentComment>, DocumentError> {
        page_limit(limit)?;
        if after.is_some_and(|v| !valid_id(v, "dcm_")) {
            return Err(DocumentError::Invalid);
        }
        let record = self.authorize(caller, id, token, false).await?;
        let mut items: Vec<_> = record
            .comments
            .into_iter()
            .filter(|r| after.is_none_or(|a| r.id.as_str() > a))
            .collect();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        let more = items.len() > usize::from(limit);
        items.truncate(usize::from(limit));
        let next = if more {
            items.last().map(|r| r.id.clone())
        } else {
            None
        };
        self.authorize(caller, id, token, false).await?;
        Ok(DocumentPage { items, next })
    }
    pub async fn shares(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<Vec<DocumentShare>, DocumentError> {
        Ok(self.owner(caller, id).await?.shares)
    }
    pub async fn links(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<Vec<DocumentLink>, DocumentError> {
        Ok(self
            .owner(caller, id)
            .await?
            .links
            .into_iter()
            .map(|l| l.metadata)
            .collect())
    }
    pub(super) async fn authorize(
        &self,
        caller: &Caller,
        id: &str,
        token: Option<&str>,
        comment: bool,
    ) -> Result<StoredDocument, DocumentError> {
        principal(caller)?;
        if !valid_id(id, "doc_") {
            return Err(DocumentError::NotFound);
        }
        let record = self.store.get(id).await?.ok_or(DocumentError::NotFound)?;
        let access = access(caller, token, &record)?;
        if comment && access == DocumentAccess::Read {
            return Err(DocumentError::NotFound);
        }
        Ok(record)
    }
    pub(super) async fn owner(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<StoredDocument, DocumentError> {
        let record = self.authorize(caller, id, None, false).await?;
        if record.metadata.owner_subject != caller.subject.as_str() {
            return Err(DocumentError::NotFound);
        }
        Ok(record)
    }
    pub(super) async fn upload(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        tokio::time::timeout(Duration::from_secs(15), self.blobs.put(key, bytes))
            .await
            .map_err(|_| DocumentError::Unavailable)?
    }
}
pub(super) fn access(
    caller: &Caller,
    token: Option<&str>,
    record: &StoredDocument,
) -> Result<DocumentAccess, DocumentError> {
    if record.metadata.owner_subject == caller.subject.as_str() {
        return Ok(DocumentAccess::Owner);
    }
    let direct = record
        .shares
        .iter()
        .find(|s| s.subject == caller.subject.as_str())
        .map(|s| s.permission);
    let linked = token
        .filter(|t| valid_token(t))
        .and_then(|t| {
            record
                .links
                .iter()
                .find(|l| l.token_sha256 == digest(t.as_bytes()))
        })
        .map(|l| l.metadata.permission);
    match (direct, linked) {
        (Some(DocumentPermission::Comment), _) | (_, Some(DocumentPermission::Comment)) => {
            Ok(DocumentAccess::Comment)
        }
        (Some(DocumentPermission::Read), _) | (_, Some(DocumentPermission::Read)) => {
            Ok(DocumentAccess::Read)
        }
        _ => Err(DocumentError::NotFound),
    }
}
pub(super) fn view(
    caller: &Caller,
    token: Option<&str>,
    record: &StoredDocument,
) -> Result<DocumentView, DocumentError> {
    let mut view = record.metadata.clone();
    view.access = access(caller, token, record)?;
    Ok(view)
}
pub(super) fn principal(caller: &Caller) -> Result<(), DocumentError> {
    if !types::valid_subject(caller.subject.as_str())
        || caller.require(Capability::UseOwnSandboxes).is_err()
    {
        return Err(DocumentError::NotFound);
    }
    Ok(())
}
pub(super) fn operation_key(key: &str) -> Result<(), DocumentError> {
    if key.is_empty() || key.len() > 255 || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(DocumentError::Invalid);
    }
    Ok(())
}
pub(super) fn valid_token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn object_key(owner: &str, id: &str, sha: &str) -> String {
    format!("documents/{}/{id}/{sha}.md", digest(owner.as_bytes()))
}
fn page_limit(limit: u16) -> Result<(), DocumentError> {
    if limit == 0 || limit > MAX_DOCUMENT_PAGE {
        Err(DocumentError::Invalid)
    } else {
        Ok(())
    }
}
fn creation_replay(
    caller: &Caller,
    record: StoredDocument,
    fingerprint: &str,
) -> Result<DocumentView, DocumentError> {
    if record.metadata.owner_subject != caller.subject.as_str() {
        return Err(DocumentError::NotFound);
    }
    if record.creation_fingerprint != fingerprint {
        return Err(DocumentError::IdempotencyConflict);
    }
    Ok(record.metadata)
}
