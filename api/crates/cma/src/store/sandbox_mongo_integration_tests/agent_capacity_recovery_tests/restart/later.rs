//! Real worker/loss consumers after a current publication. Native is a counted
//! no-input protocol stub, never a production activation qualification.
use super::*;
mod support;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_later_progress_restarts_with_current_target_without_rewriting_completion() {
    for turn in [false, true] {
        let (client, name) = database("capacity_later_restart").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (request, completed) = support::complete(&store).await;
        let original = support::snapshot(&store, &completed).await;
        let advanced = support::progress(&store, &request, turn).await;
        let before = support::snapshot(&store, &completed).await;
        assert_eq!(before.carrier, original.carrier);
        assert_eq!(before.original_publication, original.original_publication);
        assert!(advanced.revision > original.agent.revision);
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let service = Arc::new(RecoveryService::new(
            reopened.clone(),
            transactions::access(&reopened),
        ));
        let native = Native::default();
        let router = Arc::new(RecoveryReconnect::new(
            service,
            native.clone(),
            "current-restart".into(),
        ));
        let worker = support::worker(&reopened, router.clone());
        if turn {
            native.stale_confirmation.store(true, Ordering::SeqCst);
            assert_eq!(
                worker
                    .reconcile_connections(SystemTime::now())
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
            assert_eq!(support::snapshot(&store, &completed).await, before);
            native.stale_confirmation.store(false, Ordering::SeqCst);
        }
        assert_eq!(
            worker
                .reconcile_connections(SystemTime::now())
                .await
                .unwrap(),
            1
        );
        let route: Arc<dyn CapacityRecoveryReconnect> = router;
        let handler = DurableAgentConnectionLossHandler::with_capacity_recovery(
            Arc::new(reopened),
            Arc::new(OnceLock::from(route)),
        );
        handler
            .connection_lost(LiveAgentAttachmentIdentity {
                agent_id: advanced.id.clone(),
                conversation_id: completed.target.conversation_id.clone(),
                runtime_generation: completed.target.conversation_generation,
                authority_fence: advanced.work.next_fence - 1,
                nonce: 1,
            })
            .await;
        assert_eq!(
            native.attaches.load(Ordering::SeqCst),
            2 + usize::from(turn)
        );
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 2);
        for activation in native.activations.lock().unwrap().iter() {
            assert_eq!(activation.completed(), &completed);
            assert_eq!(activation.agent(), &advanced);
            assert_eq!(
                activation.target().active_turn,
                advanced.conversation.active_turn
            );
            assert_eq!(activation.writer_fence(), advanced.work.next_fence - 1);
            if turn {
                assert_ne!(
                    activation.target().input_sha256,
                    completed.target.input_sha256
                );
            }
        }
        assert_eq!(support::snapshot(&store, &completed).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_later_progress_refuses_missing_old_or_foreign_current_witness_before_io() {
    let (client, name) = database("capacity_later_refusal").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (request, completed) = support::complete(&store).await;
    let current = support::progress(&store, &request, false).await;
    let before = support::snapshot(&store, &completed).await;
    let publications = &store.segment_logs.publications;
    let row = publications
        .find_one(doc! {"result_revision": current.revision as i64})
        .await
        .unwrap()
        .unwrap();
    let id = row.get_str("_id").unwrap();
    for variant in 0..3 {
        if variant == 0 {
            publications.delete_one(doc! {"_id": id}).await.unwrap();
        } else {
            let mut changed = row.clone();
            if variant == 1 {
                changed.insert("format_version", 1_i64);
                changed.remove("agent_digest");
            } else {
                changed.insert(
                    "agent_digest",
                    mongodb::bson::Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: vec![0; 32],
                    },
                );
            }
            publications
                .replace_one(doc! {"_id": id}, changed)
                .await
                .unwrap();
        }
        let service = Arc::new(RecoveryService::new(
            store.clone(),
            transactions::access(&store),
        ));
        let native = Native::default();
        let router = Arc::new(RecoveryReconnect::new(
            service,
            native.clone(),
            "unproved".into(),
        ));
        let worker = support::worker(&store, router);
        assert_eq!(
            worker
                .reconcile_connections(SystemTime::now())
                .await
                .unwrap(),
            0
        );
        assert_eq!(native.attaches.load(Ordering::SeqCst), 0);
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.get_agent(&current.id).await.unwrap(),
            Some(current.clone())
        );
        if variant == 0 {
            publications.insert_one(row.clone()).await.unwrap();
        } else {
            publications
                .replace_one(doc! {"_id": id}, row.clone())
                .await
                .unwrap();
        }
        assert_eq!(support::snapshot(&store, &completed).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

struct RacingNative {
    native: Native,
    store: MongoSandboxStore,
    retire: bool,
}

#[async_trait::async_trait]
impl CapacityRecoveryRuntime for RacingNative {
    type Attachment = RecoveryActivation;
    async fn reattach_existing(
        &self,
        _: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        panic!("later completed work must never repeat original runtime claim")
    }
    async fn reattach_completed(
        &self,
        activation: &RecoveryActivation,
    ) -> Result<CompletedAttachment<Self::Attachment>, RecoveryError> {
        let attached = self.native.reattach_completed(activation).await?;
        if self.retire {
            let mut parent = self
                .store
                .get_sandbox(&activation.agent().sandbox_id)
                .await
                .unwrap()
                .unwrap();
            parent.status = SandboxStatus::Retiring;
            parent.revision += 1;
            self.store.sandboxes.update_one(doc! {"_id":parent.id.as_str()},
                doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()}).await.unwrap();
        } else {
            support::progress(&self.store, &activation.completed().request, false).await;
        }
        Ok(attached)
    }
    async fn acknowledge(
        &self,
        attachment: Self::Attachment,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        self.native.acknowledge(attachment, completed).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_later_progress_recaptures_parent_and_publication_before_acknowledgement() {
    for retire in [false, true] {
        let (client, name) = database("capacity_later_race").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (request, completed) = support::complete(&store).await;
        support::progress(&store, &request, false).await;
        let before = support::snapshot(&store, &completed).await;
        let native = Native::default();
        let service = Arc::new(RecoveryService::new(
            store.clone(),
            transactions::access(&store),
        ));
        let router = Arc::new(RecoveryReconnect::new(
            service,
            RacingNative {
                native: native.clone(),
                store: store.clone(),
                retire,
            },
            "race".into(),
        ));
        let worker = support::worker(&store, router);
        assert_eq!(
            worker
                .reconcile_connections(SystemTime::now())
                .await
                .unwrap(),
            0
        );
        assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
        assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
        let after = support::snapshot(&store, &completed).await;
        assert_eq!(after.carrier, before.carrier);
        assert_eq!(after.original_publication, before.original_publication);
        assert_eq!(after.agent.work.step, AgentWorkStep::None);
        assert_eq!(after.publications, before.publications + u64::from(!retire));
        client.database(&name).drop().await.unwrap();
    }
}
