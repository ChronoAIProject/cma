use super::*;
use layer2_core::sandbox::binding::*;

// Exact strict record from develop 02cde5df6, before reset-format support.
mod predecessor {
    use layer2_core::{sandbox::binding::*, SandboxId, SubjectId, WorkspaceId};
    use serde::{Deserialize, Serialize};
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ProviderBinding {
        pub binding_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub existing_agent: Option<ExistingBindingAgent>,
        /// The management principal is independent of the resource owner.
        pub manager: SubjectId,
        pub owner: SubjectId,
        pub profile_digest: String,
        pub sandbox_id: SandboxId,
        pub workspace_id: WorkspaceId,
        pub state: BindingState,
        pub level: u8,
        pub honoured: Vec<String>,
        pub refused: Vec<BindingFieldRefusal>,
        pub profile: BindingProfilePlan,
        pub key_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tool_endpoint: Option<String>,
        pub generation: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub generation_guard: Option<BindingGenerationGuard>,
        pub work: BindingWork,
        pub previous: Option<PreviousBindingEnvironment>,
        pub created_at: String,
        pub updated_at: String,
        pub revision: u64,
    }
}

fn promoted_document() -> Document {
    let mut binding = super::mongo_tests::binding();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding.reset_format = Some(ConversationResetFormat::V1);
    let successor = BindingConversation::new(
        &binding,
        "inc_successor".into(),
        super::mongo_tests::NOW.into(),
    )
    .unwrap();
    let resets = ConversationResetState {
        heads: [(
            ConversationResetState::key(1, "transport"),
            ConversationHead {
                transport_conversation_id: "transport".into(),
                incarnation: 1,
                successor,
            },
        )]
        .into(),
        receipts: Default::default(),
    };
    let mut row = binding_document(&binding).unwrap();
    row.insert(
        "conversation_resets_v1",
        crate::store::sandbox_mongo::record_bson(&resets).unwrap(),
    );
    row
}

#[test]
fn mixed_readers_accept_legacy_but_predecessor_rejects_promoted_strict_record() {
    let legacy = binding_document(&super::mongo_tests::binding()).unwrap();
    assert!(
        crate::store::sandbox_mongo::read_record::<predecessor::ProviderBinding>(&legacy).is_ok()
    );
    assert!(read_binding(legacy).is_ok());
    let promoted = promoted_document();
    assert!(read_binding(promoted.clone()).is_ok());
    let error = crate::store::sandbox_mongo::read_record::<predecessor::ProviderBinding>(&promoted)
        .unwrap_err();
    assert!(error.contains("reset_format"), "{error}");
}

#[test]
fn reader_refuses_missing_unmarked_unknown_and_malformed_reset_state() {
    let valid = promoted_document();
    for mutation in 0..7 {
        let mut row = valid.clone();
        match mutation {
            0 => {
                row.remove("conversation_resets_v1");
            }
            1 => {
                row.get_document_mut("record")
                    .unwrap()
                    .remove("reset_format");
            }
            2 => {
                row.get_document_mut("record")
                    .unwrap()
                    .insert("reset_format", "v2");
            }
            3 => {
                row.insert("conversation_resets_v1", doc! {"heads": {}, "receipts": {}});
            }
            4 => {
                row.insert(
                    "conversation_resets_v1",
                    doc! {"heads": "invalid", "receipts": {}},
                );
            }
            5 => {
                row.insert("conversation_resets_v1", mongodb::bson::Bson::Null);
            }
            _ => {
                row.remove("conversation_resets_v1");
                row.get_document_mut("record")
                    .unwrap()
                    .insert("reset_format", mongodb::bson::Bson::Null);
            }
        }
        assert!(
            matches!(
                read_binding(row.clone()),
                Err(SandboxStoreError::Corrupt(_))
            ),
            "mutation {mutation}"
        );
        assert!(
            matches!(reset::read_state(&row), Err(SandboxStoreError::Corrupt(_))),
            "mutation {mutation}"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_reset_promotes_both_scopes_and_fences_predecessor_writes() {
    let (client, database, store, binding, mapping) = super::reset_tests::fixture().await;
    let mut scope = super::mongo_tests::binding();
    scope.binding_id = "channel-default-consumer".into();
    scope.key_sha256 = "b".repeat(64);
    scope.owner = binding.owner.clone();
    scope.profile.owner = binding.owner.clone();
    store.save_binding(None, scope.clone()).await.unwrap();
    let mut request = super::reset_tests::request(&binding, &mapping, "consumer-reset");
    request.idempotency_binding_id = scope.binding_id.clone();
    let mut originals = Vec::new();
    for record in [&binding, &scope] {
        let row = store
            .bindings
            .records
            .find_one(doc! {"_id": &record.binding_id})
            .await
            .unwrap()
            .unwrap();
        assert!(
            crate::store::sandbox_mongo::read_record::<predecessor::ProviderBinding>(&row).is_ok()
        );
        originals.push(row);
    }
    assert!(matches!(
        store
            .reset_binding_conversation(request.clone(), false)
            .await,
        Err(BindingError::ResetRefused("partition_reset_disabled"))
    ));
    for row in &originals {
        assert_eq!(
            *row,
            store
                .bindings
                .records
                .find_one(doc! {"_id": row.get_str("_id").unwrap()})
                .await
                .unwrap()
                .unwrap()
        );
    }
    let receipt = store
        .reset_binding_conversation(request.clone(), true)
        .await
        .unwrap();
    for (record, stale) in [&binding, &scope].into_iter().zip(originals) {
        let row = store
            .bindings
            .records
            .find_one(doc! {"_id": &record.binding_id})
            .await
            .unwrap()
            .unwrap();
        let current = read_binding(row.clone()).unwrap();
        assert_eq!(current.revision, record.revision + 1);
        assert_eq!(current.reset_format, Some(ConversationResetFormat::V1));
        assert!(
            crate::store::sandbox_mongo::read_record::<predecessor::ProviderBinding>(&row).is_err()
        );
        // Execute the predecessor's CAS with a snapshot taken before promotion.
        let mut set = stale;
        set.remove("_id");
        set.insert("revision", integer(record.revision + 1).unwrap());
        set.get_document_mut("record")
            .unwrap()
            .insert("revision", integer(record.revision + 1).unwrap());
        let result = store.bindings.records.update_one(
            doc! {"_id": &record.binding_id, "revision": integer(record.revision).unwrap(), "document_schema_version": DOCUMENT_SCHEMA_VERSION},
            doc! {"$set": set},
        ).await.unwrap();
        assert_eq!(result.matched_count, 0);
        let mut next = current.clone();
        next.reset_format = None;
        next.advance_revision(layer2_core::now_rfc3339()).unwrap();
        assert!(matches!(
            store.save_binding(Some(current.revision), next).await,
            Err(SandboxStoreError::Conflict)
        ));
        let mut next = current.clone();
        next.advance_revision(layer2_core::now_rfc3339()).unwrap();
        store
            .save_binding(Some(current.revision), next)
            .await
            .unwrap();
        let preserved = store
            .bindings
            .records
            .find_one(doc! {"_id": &record.binding_id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            preserved.get("conversation_resets_v1"),
            row.get("conversation_resets_v1")
        );
    }
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened
            .reset_binding_conversation(request.clone(), false)
            .await
            .unwrap(),
        receipt
    );
    request.binding_id = scope.binding_id.clone();
    assert!(matches!(
        reopened.reset_binding_conversation(request, true).await,
        Err(BindingError::ResetRefused("partition_reset_key_conflict"))
    ));
    client.database(&database).drop().await.unwrap();
}
