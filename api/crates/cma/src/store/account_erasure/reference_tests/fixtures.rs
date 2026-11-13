use super::*;

pub(super) const AT: &str = "2026-11-14T00:00:00Z";
pub(super) fn document() -> StoredDocument {
    let id = "doc_11111111111111111111111111111111".to_owned();
    let sha = digest(b"# Source");
    let metadata = DocumentView {
        id: id.clone(),
        owner_subject: "survivor".into(),
        title: "Surviving title".into(),
        revision: 3,
        source_revision: 1,
        sha256: sha.clone(),
        size_bytes: 8,
        created_at: AT.into(),
        updated_at: AT.into(),
        origin_agent_id: None,
        access: DocumentAccess::Owner,
    };
    let comment = DocumentComment {
        id: "dcm_22222222222222222222222222222222".into(),
        author_subject: "erased".into(),
        body: "Private comment".into(),
        document_revision: 1,
        created_at: AT.into(),
    };
    let other = DocumentComment {
        id: "dcm_33333333333333333333333333333333".into(),
        author_subject: "other".into(),
        body: "Keep this comment".into(),
        document_revision: 1,
        created_at: AT.into(),
    };
    let value = StoredDocument {
        runtime_author_agent_id: None,
        metadata: metadata.clone(),
        title_search: metadata.title.to_lowercase(),
        revisions: vec![StoredRevision {
            metadata: DocumentRevision {
                revision: 1,
                title: metadata.title.clone(),
                sha256: sha.clone(),
                size_bytes: 8,
                created_at: AT.into(),
                author_subject: "survivor".into(),
            },
            object_key: format!("documents/{}/{id}/{sha}.md", digest(b"survivor")),
        }],
        shares: vec![
            DocumentShare {
                subject: "erased".into(),
                permission: DocumentPermission::Comment,
            },
            DocumentShare {
                subject: "other".into(),
                permission: DocumentPermission::Comment,
            },
        ],
        links: vec![],
        comments: vec![comment.clone(), other],
        receipts: BTreeMap::from([(
            "comment-receipt".into(),
            DocumentReceipt {
                fingerprint: digest(b"comment"),
                outcome: DocumentMutation {
                    document: metadata,
                    comment: Some(comment),
                    link: None,
                },
            },
        )]),
        creation_fingerprint: digest(b"create"),
    };
    value.validate().unwrap();
    value
}
