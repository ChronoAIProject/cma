use super::*;
use crate::store::{agent_log::log_document, agents::agent_document};
use mongodb::{options::ClientOptions, Client};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_admission_refuses_new_child_message_and_stop_after_native_proof() {
    let (client, name, store) = database().await;
    for change in ["child", "message", "stop"] {
        let request = fixture(&store, &format!("sbx_recovery_{change}")).await;
        let mut agent = request.snapshot.agents[0].clone();
        match change {
            "child" => {
                agent.id = layer2_core::AgentId("agt_new_recovery_child".into());
                agent.creation_key = "new-child".into();
                insert_agent(&store, &agent, NOW).await;
            }
            "message" | "stop" => {
                agent.revision += 1;
                if change == "message" {
                    agent.conversation.queued_messages.push(
                        QueuedAgentMessage::new(3, "new owner input".into(), NOW.into()).unwrap(),
                    );
                } else {
                    agent.requested_stop_reason = Some(AgentStopReason::Owner);
                    agent.work.schedule(AgentWorkStep::CloseProcess, NOW.into());
                }
                store
                    .agents
                    .replace_one(
                        doc! { "_id": agent.id.as_str() },
                        agent_document(&agent).unwrap(),
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            store
                .admit_unpublished_runtime_recovery(request.clone())
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            store
                .get_sandbox(&request.snapshot.sandbox.id)
                .await
                .unwrap()
                .unwrap(),
            request.snapshot.sandbox
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
        assert_eq!(
            store
                .sandbox_events
                .count_documents(doc! { "sandbox_id": request.snapshot.sandbox.id.as_str() })
                .await
                .unwrap(),
            0
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_log_scan_refuses_publication_and_missing_or_corrupt_history() {
    let (client, name, store) = database().await;
    for change in ["published", "missing", "gap"] {
        let request = fixture(&store, &format!("sbx_log_{change}")).await;
        let agent = &request.snapshot.agents[0];
        match change {
            "published" => {
                let entry = AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: 1,
                    appended_at: NOW.into(),
                    kind: AgentLogKind::ConversationOpened {
                        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                        runtime_generation: 2,
                    },
                };
                store
                    .agent_log
                    .replace_one(
                        doc! { "agent_id": agent.id.as_str() },
                        log_document(&entry).unwrap(),
                    )
                    .await
                    .unwrap();
            }
            "missing" => {
                store
                    .agent_log
                    .delete_many(doc! { "agent_id": agent.id.as_str() })
                    .await
                    .unwrap();
            }
            "gap" => {
                store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": agent.id.as_str() },
                        doc! { "$set": { "seq": 2_i64, "record.seq": 2_i64 } },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            store
                .unpublished_runtime_recovery_snapshot(&request.snapshot.sandbox.id)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            store
                .admit_unpublished_runtime_recovery(request.clone())
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            store
                .get_sandbox(&request.snapshot.sandbox.id)
                .await
                .unwrap()
                .unwrap(),
            request.snapshot.sandbox
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_api_services_admit_only_one_recovery_without_replay() {
    struct Verify;
    #[async_trait::async_trait]
    impl SandboxUnpublishedRecoveryVerifier for Verify {
        async fn verify(
            &self,
            snapshot: &SandboxUnpublishedRecoverySnapshot,
        ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
            let plan = serde_json::json!({ "fixture": "original" });
            Ok(SandboxUnpublishedRecoveryProof {
                sandbox_id: snapshot.sandbox.id.clone(),
                runtime_id: snapshot
                    .sandbox
                    .runtime
                    .as_ref()
                    .unwrap()
                    .runtime_id
                    .clone(),
                runtime_generation: snapshot.runtime_generation()?,
                home_identity: snapshot.home_identity()?,
                boot_id: snapshot.sandbox.runtime.as_ref().unwrap().boot_id.clone(),
                reserved_provision: None,
                observed_provision_generation: None,
                runtime_process_absent: false,
                requires_native_quiescence: true,
                provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&plan)
                    .unwrap(),
                provider_plan: plan,
                conversations: snapshot
                    .agents
                    .iter()
                    .map(|a| SandboxRecoveryConversationProof {
                        agent_id: a.id.clone(),
                        conversation_id: a.conversation.conversation_id.clone().unwrap(),
                        history_sha256: "a".repeat(64),
                    })
                    .collect(),
            })
        }
    }
    let (client, name, store) = database().await;
    let request = fixture(&store, "sbx_two_instances").await;
    let service = || {
        SandboxRuntimeUpdateService::new(std::sync::Arc::new(store.clone()), "image-v1".into())
            .with_unpublished_recovery(std::sync::Arc::new(Verify))
    };
    let (a, b) = (service(), service());
    let id = &request.snapshot.sandbox.id;
    let (a, b) = tokio::join!(
        a.recover_failed_unpublished(id),
        b.recover_failed_unpublished(id)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! { "sandbox_id": id.as_str() })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .get_agent(&request.snapshot.agents[0].id)
            .await
            .unwrap()
            .as_ref(),
        Some(&request.snapshot.agents[0])
    );
    assert!(service().recover_failed_unpublished(id).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lost_admission_response_reconciles_the_original_commit_receipt() {
    for committed in [true, false] {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("cma_recovery_unknown_{suffix}");
        let app = format!("recovery-unknown-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let request = fixture(&store, "sbx_unknown_commit").await;
        let mut data = doc! { "failCommands": ["commitTransaction"], "appName": &app };
        if committed {
            data.insert("writeConcernError", doc! { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous recovery commit" });
        } else {
            data.insert("errorCode", 50_i32);
            data.insert("errorLabels", vec!["UnknownTransactionCommitResult"]);
        }
        admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 5_i32 }, "data": data }).await.unwrap();
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let result = store
            .admit_unpublished_runtime_recovery(request.clone())
            .await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .unwrap();
        let diagnostics = output.text();
        assert!(
            diagnostics.contains("reason=UnknownCommit"),
            "{diagnostics}"
        );
        let reason = if committed {
            "reason=Success"
        } else {
            "reason=UnresolvedCommit"
        };
        assert!(
            diagnostics
                .lines()
                .any(|line| line.contains("stage=ReconcileCommit") && line.contains(reason)),
            "{diagnostics}"
        );
        assert_eq!(result.is_ok(), committed, "{result:?}");
        if !committed {
            assert_eq!(
                store
                    .get_sandbox(&request.snapshot.sandbox.id)
                    .await
                    .unwrap()
                    .unwrap(),
                request.snapshot.sandbox
            );
            assert_eq!(
                store
                    .sandbox_events
                    .count_documents(doc! { "sandbox_id": request.snapshot.sandbox.id.as_str() })
                    .await
                    .unwrap(),
                0
            );
            // The unresolved attempt never claimed success or invented a new receipt.
            client.database(&name).drop().await.unwrap();
            continue;
        }
        let recovered = result.unwrap();
        assert_eq!(
            store
                .get_sandbox(&request.snapshot.sandbox.id)
                .await
                .unwrap()
                .unwrap(),
            recovered
        );
        assert_eq!(
            recovered.lifecycle.suspended_runtime.as_deref(),
            request.snapshot.sandbox.runtime.as_ref()
        );
        assert_eq!(
            store
                .get_agent(&request.snapshot.agents[0].id)
                .await
                .unwrap()
                .as_ref(),
            Some(&request.snapshot.agents[0])
        );
        assert!(store
            .admit_unpublished_runtime_recovery(request.clone())
            .await
            .is_err());
        assert_eq!(
            store
                .sandbox_events
                .count_documents(doc! { "sandbox_id": request.snapshot.sandbox.id.as_str() })
                .await
                .unwrap(),
            1
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[derive(Clone, Default)]
struct Output(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
