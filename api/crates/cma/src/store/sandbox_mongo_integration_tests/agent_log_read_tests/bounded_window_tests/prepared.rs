use super::*;
use crate::store::agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_reads::integrity::PreparedIntegrity;
use crate::store::agents::agent_replacement_fields;

impl Fixture {
    async fn prepare(mut self) -> Self {
        let mut agent = self.store.get_agent(&self.id).await.unwrap().unwrap();
        let count = agent.conversation.last_seq;
        assert!(count >= 6);
        agent.status = AgentStatus::Ready;
        agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
        agent.conversation.conversation_id = Some("cnv_window".into());
        agent.conversation.runtime_generation = Some(1);
        agent.revision += 1;
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.creation_worktree_id().clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "cnv_window".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
        ];
        kinds.resize(
            count as usize - 1,
            AgentLogKind::AgentMessage {
                content: "x".repeat(4096),
            },
        );
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        self.store
            .agent_log
            .delete_many(doc! { "agent_id": self.id.as_str() })
            .await
            .unwrap();
        for (batch, kinds) in kinds.chunks(200).enumerate() {
            let rows = kinds.iter().enumerate().map(|(offset, kind)| {
                log_document(&AgentLogEntry {
                    agent_id: self.id.clone(),
                    seq: (batch * 200 + offset + 1) as u64,
                    appended_at: NOW.into(),
                    kind: kind.clone(),
                })
                .unwrap()
            });
            self.store.agent_log.insert_many(rows).await.unwrap();
        }
        let mut fields = agent_replacement_fields(&agent).unwrap();
        fields.insert("last_seq", count as i64);
        self.store
            .agents
            .update_one(
                doc! { "_id": self.id.as_str() },
                doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
            )
            .await
            .unwrap();
        self.store = self
            .store
            .prepare_bounded_admission(std::slice::from_ref(&self.id))
            .await
            .unwrap();
        self
    }

    fn reset_commands(&self) -> u64 {
        self.commands.lock().unwrap().clear();
        self.checkouts.load(Ordering::SeqCst)
    }

    fn assert_read_only(&self) {
        for command in self.commands.lock().unwrap().iter() {
            assert!(
                ![
                    "insert",
                    "update",
                    "delete",
                    "findAndModify",
                    "startTransaction",
                    "abortTransaction",
                    "commitTransaction"
                ]
                .iter()
                .any(|name| command.contains_key(name)),
                "public read must not mutate or operate a transaction"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn prepared_windows_measure_total_proof_and_disabled_fallback_checkouts() {
    for history in [1_000, 10_000] {
        let mut fixture = Fixture::new(history).await.prepare().await;
        telemetry::isolated_test(async {
            let window = Window::Page { after: history - 200, limit: 200 };
            let before = fixture.reset_commands();
            let started = std::time::Instant::now();
            let legacy = fixture.read(window, &LegacyIntegrity).await.unwrap();
            let legacy_us = started.elapsed().as_micros();
            let legacy_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(telemetry::snapshot(AGENT_LOG)[2], history);
            let before = fixture.reset_commands();
            let started = std::time::Instant::now();
            let bounded = fixture.store.agent_log(&fixture.id, history - 200, 200).await.unwrap();
            let enabled_us = started.elapsed().as_micros();
            let enabled_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(bounded, legacy);
            assert_eq!(enabled_checkouts, 4, "Agent + tail + extra-tail + window");
            assert_eq!(telemetry::snapshot(AGENT_LOG)[2], history + 201);
            fixture.assert_read_only();
            let commands = fixture.commands.lock().unwrap().clone();
            assert_eq!(commands.len(), 4);
            for command in &commands {
                assert!(command_uses_snapshot_read(command));
                assert_eq!(commands[0].get_document("lsid").unwrap(), command.get_document("lsid").unwrap());
            }
            let before = fixture.reset_commands();
            assert!(fixture.store.agent_log(&fixture.id, history, 200).await.unwrap().is_empty());
            assert_eq!(fixture.checkouts.load(Ordering::SeqCst) - before, 3);
            fixture.assert_read_only();
            // Restart with the rollout cohort disabled, preserving prepared metadata.
            fixture.store = MongoSandboxStore::open(&fixture.client, &fixture.name).await.unwrap();
            let before = fixture.reset_commands();
            assert_eq!(fixture.read(window, &PreparedIntegrity).await.unwrap(), legacy);
            let disabled_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(disabled_checkouts, legacy_checkouts + 2, "disabled proof cost must remain visible");
            fixture.assert_read_only();
            eprintln!("public_window_total history={history} page=200 legacy_checkouts={legacy_checkouts} enabled_checkouts={enabled_checkouts} disabled_prepared_checkouts={disabled_checkouts} enabled_log_documents=201 legacy_us={legacy_us} enabled_us={enabled_us}; local unoptimized single sample, not a production latency forecast");
        }).await;
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_window_metadata_fallback_and_refusal_never_write_or_repair() {
    for case in [
        "missing",
        "stale",
        "unsupported",
        "quarantine",
        "tail_hash",
        "extra_tail",
        "gap",
        "trusted_gap",
        "trusted_corrupt",
        "trusted_creation",
        "lineage",
    ] {
        let fixture = Fixture::new(200).await.prepare().await;
        let filter = doc! { "_id": fixture.id.as_str() };
        let update = match case {
            "missing" => Some(doc! { "$unset": { INTEGRITY_FIELD: "" } }),
            "stale" => Some(doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } }),
            "unsupported" => Some(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 } }),
            "quarantine" => Some(doc! { "$set": { QUARANTINE_FIELD: {} } }),
            "tail_hash" => {
                fixture
                    .store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": fixture.id.as_str(), "seq": 200_i64 },
                        doc! { "$set": { "record.appended_at": TWO_MINUTES } },
                    )
                    .await
                    .unwrap();
                None
            }
            "extra_tail" => {
                fixture
                    .store
                    .agent_log
                    .insert_one(log_document(&entry(&fixture.id, 201)).unwrap())
                    .await
                    .unwrap();
                None
            }
            "gap" => {
                fixture
                    .store
                    .agent_log
                    .delete_one(doc! { "agent_id": fixture.id.as_str(), "seq": 100_i64 })
                    .await
                    .unwrap();
                Some(doc! { "$unset": { INTEGRITY_FIELD: "" } })
            }
            "trusted_gap" => {
                fixture
                    .store
                    .agent_log
                    .delete_one(doc! {
                        "agent_id": fixture.id.as_str(), "seq": 198_i64
                    })
                    .await
                    .unwrap();
                None
            }
            "trusted_corrupt" => {
                fixture
                    .store
                    .agent_log
                    .update_one(
                        doc! {
                            "agent_id": fixture.id.as_str(), "seq": 198_i64
                        },
                        doc! { "$set": { "record.agent_id": "agt_foreign" } },
                    )
                    .await
                    .unwrap();
                None
            }
            "trusted_creation" => {
                let agent = fixture.store.get_agent(&fixture.id).await.unwrap().unwrap();
                fixture
                    .store
                    .agent_log
                    .update_one(
                        doc! {
                            "agent_id": fixture.id.as_str(), "seq": 198_i64
                        },
                        doc! { "$set": { "record.kind": {
                            "type": "created", "workspace_id": agent.workspace_id.as_str(),
                            "worktree_id": agent.creation_worktree_id().as_str(),
                        } } },
                    )
                    .await
                    .unwrap();
                None
            }
            "lineage" => {
                fixture
                    .store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": fixture.id.as_str(), "seq": 1_i64 },
                        doc! { "$set": { "record.kind.workspace_id": "wks_foreign" } },
                    )
                    .await
                    .unwrap();
                Some(doc! { "$unset": { INTEGRITY_FIELD: "" } })
            }
            _ => unreachable!(),
        };
        if let Some(update) = update {
            fixture
                .store
                .agents
                .update_one(filter.clone(), update)
                .await
                .unwrap();
        }
        let before = fixture
            .store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        fixture.reset_commands();
        let result = fixture.store.agent_log(&fixture.id, 195, 5).await;
        if matches!(case, "missing" | "stale" | "unsupported") {
            assert_eq!(result.unwrap().len(), 5, "{case}");
        } else {
            assert!(
                matches!(
                    result,
                    Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict)
                ),
                "{case}: {result:?}"
            );
        }
        fixture.assert_read_only();
        assert_eq!(
            fixture
                .store
                .agents
                .find_one(filter)
                .await
                .unwrap()
                .unwrap(),
            before,
            "{case}"
        );
        fixture.cleanup().await;
    }
}

struct AppendAfterProof;
#[async_trait::async_trait]
impl LogReadIntegrity for AppendAfterProof {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        let head = PreparedIntegrity.read_head(store, id, session).await?;
        store
            .append_agent_log(
                id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                },
            )
            .await?;
        Ok(head)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn prepared_window_keeps_snapshot_after_a_real_writer_advances() {
    let fixture = Fixture::new(200).await.prepare().await;
    let window = Window::Page {
        after: 195,
        limit: 10,
    };
    let captured = fixture.read(window, &AppendAfterProof).await.unwrap();
    assert_eq!(captured.len(), 5);
    assert_eq!(captured.last().unwrap().seq, 200);
    let next = fixture.store.agent_log(&fixture.id, 195, 10).await.unwrap();
    assert_eq!(next.len(), 6);
    assert_eq!(next.last().unwrap().seq, 201);
    fixture.cleanup().await;
}

mod baseline;
mod profile;

mod fleet;
