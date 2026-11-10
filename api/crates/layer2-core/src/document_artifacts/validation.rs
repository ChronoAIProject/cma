use super::*;
use std::collections::BTreeSet;
impl StoredDocument {
    pub fn validate(&self) -> Result<(), DocumentError> {
        let m = &self.metadata;
        if self
            .runtime_author_agent_id
            .as_ref()
            .is_some_and(|id| !valid_id(id, "agt_") || m.origin_agent_id.as_ref() != Some(id))
        {
            return Err(DocumentError::Corrupt);
        }
        if !valid_id(&m.id, "doc_")
            || !types::valid_subject(&m.owner_subject)
            || !types::valid_title(&m.title)
            || self.title_search != m.title.to_lowercase()
            || m.revision == 0
            || m.revision > i64::MAX as u64
            || m.source_revision > m.revision
            || m.access != DocumentAccess::Owner
            || self.revisions.is_empty()
            || self.revisions.len() > MAX_REVISIONS
            || self.shares.len() > MAX_GRANTS
            || self.links.len() > MAX_LINKS
            || self.comments.len() > MAX_COMMENTS
            || self.receipts.len() + self.shares.len() + self.links.len()
                > MAX_RECEIPTS + MAX_GRANTS + MAX_LINKS
            || crate::seconds_from_rfc3339(&m.created_at).is_none()
            || crate::seconds_from_rfc3339(&m.updated_at).is_none()
        {
            return Err(DocumentError::Corrupt);
        }
        let last = self.revisions.last().ok_or(DocumentError::Corrupt)?;
        if last.metadata.revision != m.source_revision
            || last.metadata.title != m.title
            || last.metadata.sha256 != m.sha256
            || last.metadata.size_bytes != m.size_bytes
        {
            return Err(DocumentError::Corrupt);
        }
        let mut prior = 0;
        for r in &self.revisions {
            let s = &r.metadata;
            if s.revision <= prior
                || s.revision > m.revision
                || !service::valid_token(&s.sha256)
                || s.size_bytes == 0
                || s.size_bytes > MAX_DOCUMENT_BYTES as u64
                || !types::valid_title(&s.title)
                || !types::valid_subject(&s.author_subject)
                || r.object_key != service::object_key(&m.owner_subject, &m.id, &s.sha256)
            {
                return Err(DocumentError::Corrupt);
            }
            prior = s.revision;
        }
        let mut ids = BTreeSet::new();
        for share in &self.shares {
            if !types::valid_subject(&share.subject)
                || share.subject == m.owner_subject
                || !ids.insert(&share.subject)
            {
                return Err(DocumentError::Corrupt);
            }
        }
        let mut ids = BTreeSet::new();
        for link in &self.links {
            if !valid_id(&link.metadata.id, "dln_")
                || !service::valid_token(&link.token_sha256)
                || !ids.insert(&link.metadata.id)
            {
                return Err(DocumentError::Corrupt);
            }
        }
        let mut ids = BTreeSet::new();
        for comment in &self.comments {
            if !valid_id(&comment.id, "dcm_")
                || !ids.insert(&comment.id)
                || !types::valid_subject(&comment.author_subject)
                || comment.body.trim().is_empty()
                || comment.body.len() > 8192
                || comment.body.contains('\0')
                || !self
                    .revisions
                    .iter()
                    .any(|r| r.metadata.revision == comment.document_revision)
            {
                return Err(DocumentError::Corrupt);
            }
        }
        // Each removed grant/link releases enough space for its revoke receipt.
        // The absolute ceiling remains below 13 MiB, including all reserves.
        let removed = MAX_GRANTS + MAX_LINKS - self.shares.len() - self.links.len();
        if self.metadata_bytes()? > MAX_METADATA_BYTES + removed * REVOCATION_RECEIPT_BYTES {
            return Err(DocumentError::Capacity);
        }
        Ok(())
    }

    pub(super) fn metadata_bytes(&self) -> Result<usize, DocumentError> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .map_err(|_| DocumentError::Corrupt)
    }
}
