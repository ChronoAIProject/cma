//! Durable owner-scoped chat hosts in MongoDB.

use futures_util::TryStreamExt;
use layer2_core::{
    store::{ChatHostStore, Page, PageCursor, StoreError},
    ChatHostId, ChatHostListQuery, ChatHostRecord, ChatHostStatus, SubjectId,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::error::{Error, ErrorKind, WriteFailure};
use mongodb::options::{IndexOptions, ReturnDocument};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

const CHAT_HOSTS: &str = "chat_hosts";
const DUPLICATE_KEY: i32 = 11000;

/// Queryable fields surround the complete aggregate. Reads verify both copies
/// before returning authority to Layer 2, so index corruption cannot authorize
/// provider work for a different owner or lifecycle state.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatHostDoc {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    status: ChatHostStatus,
    created_at: String,
    revision: i64,
    record: ChatHostRecord,
}

fn status_name(status: ChatHostStatus) -> &'static str {
    match status {
        ChatHostStatus::Provisioning => "provisioning",
        ChatHostStatus::Active => "active",
        ChatHostStatus::Pausing => "pausing",
        ChatHostStatus::Paused => "paused",
        ChatHostStatus::Reviving => "reviving",
        ChatHostStatus::Retired => "retired",
    }
}

fn to_doc(record: &ChatHostRecord, stored_revision: u64) -> Result<ChatHostDoc, StoreError> {
    record
        .validate()
        .map_err(|error| StoreError::Malformed(format!("chat host: {error}")))?;
    let revision = i64::try_from(stored_revision)
        .map_err(|_| StoreError::Malformed("chat host revision exceeds BSON range".into()))?;
    let mut stored = record.clone();
    stored.revision = stored_revision;
    Ok(ChatHostDoc {
        id: stored.id.0.clone(),
        owner: stored.owner.0.clone(),
        status: stored.status,
        created_at: stored.created_at.clone(),
        revision,
        record: stored,
    })
}

fn from_doc(doc: ChatHostDoc) -> Result<ChatHostRecord, StoreError> {
    let revision = u64::try_from(doc.revision)
        .map_err(|_| StoreError::Malformed("chat host revision must not be negative".into()))?;
    if doc.id != doc.record.id.0
        || doc.owner != doc.record.owner.0
        || doc.status != doc.record.status
        || doc.created_at != doc.record.created_at
        || revision != doc.record.revision
    {
        return Err(StoreError::Malformed(
            "chat host query fields disagree with the stored aggregate".into(),
        ));
    }
    doc.record
        .validate()
        .map_err(|error| StoreError::Malformed(format!("chat host: {error}")))?;
    Ok(doc.record)
}

fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

fn chat_host_indexes() -> [IndexModel; 3] {
    [
        IndexModel::builder()
            .keys(doc! { "owner": 1 })
            .options(
                IndexOptions::builder()
                    .name(Some("chat_hosts_by_owner".to_string()))
                    .unique(true)
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "created_at": -1, "_id": -1 })
            .options(
                IndexOptions::builder()
                    .name(Some("chat_hosts_newest".to_string()))
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "status": 1, "created_at": -1, "_id": -1 })
            .options(
                IndexOptions::builder()
                    .name(Some("chat_hosts_candidates".to_string()))
                    .build(),
            )
            .build(),
    ]
}

fn page_filter(query: &ChatHostListQuery) -> Document {
    let mut filter = Document::new();
    if !query.statuses.is_empty() {
        filter.insert(
            "status",
            doc! {
                "$in": query
                    .statuses
                    .iter()
                    .copied()
                    .map(status_name)
                    .map(|status| Bson::String(status.to_string()))
                    .collect::<Vec<_>>()
            },
        );
    }
    if let Some(cursor) = query.page.cursor() {
        filter.insert(
            "$or",
            vec![
                doc! { "created_at": { "$lt": &cursor.created_at } },
                doc! {
                    "created_at": &cursor.created_at,
                    "_id": { "$lt": &cursor.id },
                },
            ],
        );
    }
    filter
}

fn page_fetch_limit(query: &ChatHostListQuery) -> i64 {
    i64::from(query.page.limit().saturating_add(1))
}

/// Mongo implementation of the one-host-per-owner aggregate store.
pub struct MongoChatHostStore {
    hosts: Collection<ChatHostDoc>,
}

impl MongoChatHostStore {
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let store = Self {
            hosts: client.database(database).collection(CHAT_HOSTS),
        };
        for index in chat_host_indexes() {
            store
                .hosts
                .create_index(index)
                .await
                .map_err(|error| StoreError::Unavailable(format!("chat host index: {error}")))?;
        }
        tracing::info!(database, "chat host store ready");
        Ok(store)
    }

    async fn list_query(
        &self,
        query: &ChatHostListQuery,
    ) -> Result<Page<ChatHostRecord>, StoreError> {
        let mut cursor = self
            .hosts
            .find(page_filter(query))
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(page_fetch_limit(query))
            .await
            .map_err(|_| StoreError::Unavailable("list chat hosts: database read failed".into()))?;
        let mut items = Vec::new();
        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|_| StoreError::Unavailable("chat host cursor: database read failed".into()))?
        {
            items.push(from_doc(doc)?);
        }
        let has_more = items.len() > query.page.limit() as usize;
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            let last = items
                .last()
                .expect("a non-empty bounded chat-host page has a last row");
            PageCursor::new(&last.created_at, last.id.as_str())
        });
        Ok(Page { items, next })
    }
}

#[async_trait::async_trait]
impl ChatHostStore for MongoChatHostStore {
    async fn create_if_absent(
        &self,
        candidate: &ChatHostRecord,
    ) -> Result<ChatHostRecord, StoreError> {
        if candidate.revision != 0 {
            return Err(StoreError::Malformed(
                "a new chat host must have revision zero".into(),
            ));
        }
        let doc = to_doc(candidate, 1)?;
        match self.hosts.insert_one(&doc).await {
            Ok(_) => from_doc(doc),
            Err(error) if is_duplicate_key(&error) => {
                self.get_for_owner(&candidate.owner).await?.ok_or_else(|| {
                    StoreError::Unavailable(
                        "chat host insert raced but the winning owner record was unavailable"
                            .into(),
                    )
                })
            }
            Err(_) => Err(StoreError::Unavailable(
                "create chat host: database write failed".into(),
            )),
        }
    }

    async fn get(&self, id: &ChatHostId) -> Result<Option<ChatHostRecord>, StoreError> {
        self.hosts
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|_| StoreError::Unavailable("get chat host: database read failed".into()))?
            .map(from_doc)
            .transpose()
    }

    async fn get_for_owner(&self, owner: &SubjectId) -> Result<Option<ChatHostRecord>, StoreError> {
        self.hosts
            .find_one(doc! { "owner": owner.as_str() })
            .await
            .map_err(|_| {
                StoreError::Unavailable("get owner chat host: database read failed".into())
            })?
            .map(from_doc)
            .transpose()
    }

    async fn update_if_revision(
        &self,
        expected: u64,
        replacement: &ChatHostRecord,
    ) -> Result<Option<ChatHostRecord>, StoreError> {
        if replacement.revision != expected {
            return Err(StoreError::Malformed(
                "chat host replacement must carry the expected revision".into(),
            ));
        }
        let Ok(expected_bson) = i64::try_from(expected) else {
            return Ok(None);
        };
        let next = expected
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("chat host revision overflow".into()))?;
        let replacement = to_doc(replacement, next)?;
        self.hosts
            .find_one_and_replace(
                doc! { "_id": &replacement.id, "revision": expected_bson },
                replacement,
            )
            .return_document(ReturnDocument::After)
            .upsert(false)
            .await
            .map_err(|_| StoreError::Unavailable("update chat host: database write failed".into()))?
            .map(from_doc)
            .transpose()
    }

    async fn list(&self, query: &ChatHostListQuery) -> Result<Page<ChatHostRecord>, StoreError> {
        self.list_query(query).await
    }

    async fn list_candidates(
        &self,
        query: &ChatHostListQuery,
    ) -> Result<Page<ChatHostRecord>, StoreError> {
        self.list_query(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{ChatSettings, ChatSettingsSnapshot, KernelId};

    fn host() -> ChatHostRecord {
        let owner = SubjectId("subject-chat".into());
        ChatHostRecord {
            id: ChatHostId::for_owner(&owner),
            owner,
            status: ChatHostStatus::Provisioning,
            pinned_settings: ChatSettingsSnapshot::new(
                9,
                ChatSettings {
                    enabled: true,
                    ..ChatSettings::default()
                },
            )
            .unwrap(),
            platform_briefing: "Pinned deployment briefing\n".into(),
            kernel: KernelId::new("codex"),
            runtime_id: None,
            runtime_version: None,
            runtime_generation: 1,
            credential_generation: 0,
            workspace_authority: None,
            threads: Vec::new(),
            threads_created: 0,
            checkpoint: None,
            checkpoint_sequence: 0,
            paused_at: None,
            revived_at: None,
            rotated_at: None,
            retired_at: None,
            last_transition_claim: Some("claim-1".into()),
            transition_started_at: Some("2026-09-01T00:00:00Z".into()),
            last_failure: None,
            last_operator_note: None,
            revive_cause: None,
            revive_phase: None,
            revive_failed_threads: Vec::new(),
            prompt_admission_sequence: 0,
            prompt_admissions: Vec::new(),
            rotations: 0,
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            revision: 0,
        }
    }

    #[test]
    fn document_round_trip_pins_the_store_revision_and_query_fields() {
        let original = host();
        let stored = to_doc(&original, 7).expect("encodes");
        assert_eq!(stored.id, original.id.0);
        assert_eq!(stored.owner, original.owner.0);
        assert_eq!(stored.status, original.status);
        assert_eq!(stored.revision, 7);
        let decoded = from_doc(stored).expect("decodes");
        assert_eq!(decoded.revision, 7);
        assert_eq!(decoded.pinned_settings, original.pinned_settings);
    }

    #[test]
    fn disagreement_in_any_duplicated_authority_field_is_malformed() {
        let mut document = to_doc(&host(), 1).unwrap();
        document.owner = "different-owner".into();
        assert!(matches!(from_doc(document), Err(StoreError::Malformed(_))));

        let mut document = to_doc(&host(), 1).unwrap();
        document.status = ChatHostStatus::Active;
        assert!(matches!(from_doc(document), Err(StoreError::Malformed(_))));

        let mut document = to_doc(&host(), 1).unwrap();
        document.revision = 2;
        assert!(matches!(from_doc(document), Err(StoreError::Malformed(_))));
    }

    #[test]
    fn indexes_enforce_one_owner_and_cover_inventory_and_worker_queries() {
        let indexes = chat_host_indexes();
        assert_eq!(indexes[0].keys, doc! { "owner": 1 });
        assert_eq!(indexes[0].options.as_ref().unwrap().unique, Some(true));
        assert_eq!(indexes[1].keys, doc! { "created_at": -1, "_id": -1 });
        assert_eq!(
            indexes[2].keys,
            doc! { "status": 1, "created_at": -1, "_id": -1 }
        );
    }

    #[test]
    fn candidate_filter_is_bounded_status_aware_and_uses_the_id_tiebreaker() {
        let query = ChatHostListQuery::new(
            layer2_core::store::PageRequest::new(
                5,
                Some(PageCursor::new("2026-09-01T00:00:00Z", "chho_last")),
            ),
            vec![ChatHostStatus::Provisioning, ChatHostStatus::Paused],
        );
        assert_eq!(page_fetch_limit(&query), 6);
        assert_eq!(
            page_filter(&query),
            doc! {
                "status": { "$in": ["provisioning", "paused"] },
                "$or": [
                    { "created_at": { "$lt": "2026-09-01T00:00:00Z" } },
                    {
                        "created_at": "2026-09-01T00:00:00Z",
                        "_id": { "$lt": "chho_last" },
                    },
                ],
            }
        );
    }
}
