use super::*;
use layer2_core::bridge::outbox::OutboxError;

struct ChangeAfterObservation {
    collection: mongodb::Collection<Document>,
    filter: Document,
    update: Document,
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for ChangeAfterObservation {
    async fn validate_connection(
        &self,
        owner: &layer2_core::resource_access::ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        let validation = Authority.validate_connection(owner, config).await?;
        let changed = self
            .collection
            .update_one(self.filter.clone(), self.update.clone())
            .await
            .unwrap();
        assert_eq!(changed.matched_count, 1);
        Ok(validation)
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_fences_runtime_and_session_changed_after_initial_source_observation(
) {
    for change in ["runtime", "session", "turn"] {
        let f = ReplyFixture::new().await;
        consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
        let session = f.tools.authenticate(&f.credential).await.unwrap();
        let database = f.client.database(&f.database);
        let agent = f.command.receipt.envelope.agent_id.as_str();
        let (collection, filter, update) = match change {
            "session" => (
                "bridge_channel_capabilities",
                doc! {"record.agent_id":agent,"revoked":false},
                doc! {"$set":{"revoked":true,"record.revoked":true},"$inc":{"revision":1_i64,"record.revision":1_i64}},
            ),
            "runtime" => (
                "agents",
                doc! {"_id":agent},
                doc! {"$inc":{"record.conversation.runtime_generation":1_i64}},
            ),
            "turn" => (
                "agents",
                doc! {"_id":agent},
                doc! {"$inc":{"record.conversation.active_turn":1_i64}},
            ),
            _ => unreachable!(),
        };
        let validator = Arc::new(ChangeAfterObservation {
            collection: database.collection(collection),
            filter,
            update,
        });
        let store = crate::store::MongoBridgeReplyToolStore::new(store(&f).await, validator)
            .await
            .unwrap();
        assert!(
            store.control_context(&session).await.is_err(),
            "{change} must invalidate the original proof"
        );
        assert!(rows(&f).await.is_empty());
        assert!(f.items().await.is_empty());
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_grant_revocation_and_reset_hold_old_reference_without_send() {
    for revoke in [false, true] {
        let f = ReplyFixture::new().await;
        consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
        let _ = read(&f).await;
        let retained = rows(&f).await;
        if revoke {
            let bridge = f
                .bridges
                .bridge(&f.inbox.pin.bridge_id)
                .await
                .unwrap()
                .unwrap();
            f.invokes
                .revoke(
                    &caller(),
                    None,
                    RevokeBridgeInvokeGrant {
                        grant_id: f.grant.id.clone(),
                        expected_bridge_revision: bridge.revision,
                        expected_revision: 1,
                        idempotency_key: "reference-invoke-revoke".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            let (admission, authority, management) = proposal(
                &f.svc,
                &f.bridges,
                &f.allocation.pin.bridge_id,
                "12345",
                Some(&f.allocation),
            )
            .await;
            f.allocations
                .reserve(
                    AllocationReservation::reset(
                        admission,
                        authority,
                        f.allocation.clone(),
                        management,
                        now(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            assert!(f.context().await.starts_with("brctx_"));
        }
        assert!(read(&f).await["source_control_reference"].is_null());
        assert_eq!(rows(&f).await, retained);
        assert!(f.items().await.is_empty());
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_unknown_result_recovers_same_committed_mapping_without_overwrite(
) {
    let f = ReplyFixture::new().await;
    consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    let session = f.tools.authenticate(&f.credential).await.unwrap();
    // Lose the caller's successful result; a fresh store/process must recover
    // the mapping without receiving either its random value or an effect key.
    let _ = f.replies.control_context(&session).await.unwrap();
    let original = rows(&f).await;
    let reopened =
        crate::store::MongoBridgeReplyToolStore::new(store(&f).await, Arc::new(Authority))
            .await
            .unwrap();
    assert_eq!(
        reopened.control_context(&session).await.unwrap(),
        Some(record(&original[0]).reference)
    );
    assert_eq!(rows(&f).await, original);
    let table = f
        .client
        .database(&f.database)
        .collection::<Document>("bridge_channel_control_references");
    table
        .update_one(
            doc! {"_id":original[0].get_str("_id").unwrap()},
            doc! {"$set":{"record.binding.sender_id":"copied-other-sender"}},
        )
        .await
        .unwrap();
    assert_eq!(
        reopened.control_context(&session).await,
        Err(OutboxError::Corrupt)
    );
    assert_eq!(rows(&f).await.len(), 1);
    assert!(f.items().await.is_empty());
    f.close().await;
}
