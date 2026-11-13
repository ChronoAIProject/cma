use super::*;

struct AppendAfterCapture<'a>(&'a Agent);

#[async_trait::async_trait]
impl LogReadIntegrity for AppendAfterCapture<'_> {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        let head = crate::store::agent_log_reads::integrity::CapturedIntegrity(self.0)
            .read_head(store, id, session)
            .await?;
        store
            .append_agent_log(
                id,
                NOW.into(),
                AgentLogKind::ModeChanged {
                    mode: "after-capture".into(),
                },
            )
            .await?;
        Ok(head)
    }
}

async fn fixture(mode: &str) -> Fixture {
    let mut fixture = Fixture::new(260).await.prepare().await;
    if mode == "legacy" {
        fixture
            .store
            .agents
            .update_one(
                doc! {"_id":fixture.id.as_str()},
                doc! {"$unset":{INTEGRITY_FIELD:""}},
            )
            .await
            .unwrap();
    }
    if mode != "prepared" {
        fixture.store = MongoSandboxStore::open(&fixture.client, &fixture.name)
            .await
            .unwrap();
    }
    fixture
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_prefix_proof_removes_only_the_reopened_tail_queries() {
    for mode in ["legacy", "prepared", "disabled_prepared"] {
        let fixture = fixture(mode).await;
        let mut captured = fixture.store.get_agent(&fixture.id).await.unwrap().unwrap();
        for through in [260, 200] {
            captured.conversation.last_seq = through;
            let before = fixture.reset_commands();
            let started = std::time::Instant::now();
            let original = fixture
                .store
                .agent_log_prefix(&captured.id, through)
                .await
                .unwrap();
            let tail = fixture
                .store
                .agent_log_entry(&captured.id, through + 1)
                .await
                .unwrap();
            if let Some(tail) = tail {
                let current = fixture
                    .store
                    .get_agent(&captured.id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(current.owner, captured.owner);
                assert_eq!(current.sandbox_id, captured.sandbox_id);
                assert_eq!(current.workspace_id, captured.workspace_id);
                assert_eq!(current.worktree_id, captured.worktree_id);
                assert!(current.conversation.last_seq >= tail.seq);
            }
            let original_us = started.elapsed().as_micros();
            let original_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            fixture.assert_read_only();
            let before = fixture.reset_commands();
            let started = std::time::Instant::now();
            let proven = fixture
                .store
                .agent_log_prefix_with_tail(&captured)
                .await
                .unwrap()
                .unwrap();
            let proven_us = started.elapsed().as_micros();
            let proven_checkouts = fixture.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(proven, original);
            // Raw legacy reopening reads one Agent. Prepared metadata adds two
            // tail-proof reads. An older capture adds a point and current Agent.
            let saved = if mode == "legacy" { 1 } else { 3 } + if through < 260 { 2 } else { 0 };
            assert_eq!(original_checkouts - proven_checkouts, saved);
            fixture.assert_read_only();
            let commands = fixture.commands.lock().unwrap().clone();
            assert_eq!(commands.len() as u64, proven_checkouts);
            for command in &commands {
                assert_eq!(
                    commands[0].get_document("lsid").unwrap(),
                    command.get_document("lsid").unwrap()
                );
                if command.contains_key("find") {
                    assert!(command_uses_snapshot_read(command));
                }
            }
            eprintln!("complete_prefix mode={mode} through={through} current=260 before_checkouts={original_checkouts} after_checkouts={proven_checkouts} before_us={original_us} after_us={proven_us}; local fixture only, not production latency");
        }
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_prefix_proof_binds_captured_identity_and_preserves_concurrent_append() {
    let fixture = fixture("legacy").await;
    let captured = fixture.store.get_agent(&fixture.id).await.unwrap().unwrap();
    let prefix = fixture
        .store
        .agent_log_prefix_with_tail(&captured)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .read(
                Window::Prefix {
                    through: captured.conversation.last_seq
                },
                &AppendAfterCapture(&captured)
            )
            .await
            .unwrap(),
        prefix
    );
    fixture
        .store
        .append_agent_log(
            &captured.id,
            NOW.into(),
            AgentLogKind::ModeChanged {
                mode: "next".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .agent_log_prefix_with_tail(&captured)
            .await
            .unwrap()
            .unwrap(),
        prefix
    );
    for field in [
        "owner",
        "sandbox",
        "workspace",
        "worktree",
        "incarnation",
        "missing",
        "future",
    ] {
        let mut wrong = captured.clone();
        match field {
            "owner" => wrong.owner = SubjectId::new("other-owner"),
            "sandbox" => wrong.sandbox_id = SandboxId("other-sandbox".into()),
            "workspace" => wrong.workspace_id = WorkspaceId("other-workspace".into()),
            "worktree" => wrong.worktree_id = WorktreeId("other-worktree".into()),
            "incarnation" => wrong.created_at = TWO_MINUTES.into(),
            "missing" => wrong.id = AgentId("missing".into()),
            "future" => wrong.conversation.last_seq += 3,
            _ => unreachable!(),
        }
        fixture.reset_commands();
        assert!(
            fixture
                .store
                .agent_log_prefix_with_tail(&wrong)
                .await
                .is_err(),
            "{field}"
        );
        fixture.assert_read_only();
    }
    // Deleting/recreating the durable ID cannot certify the old incarnation.
    fixture
        .store
        .agents
        .update_one(
            doc! {"_id":captured.id.as_str()},
            doc! {"$set":{"record.created_at":TWO_MINUTES}},
        )
        .await
        .unwrap();
    assert!(fixture
        .store
        .agent_log_prefix_with_tail(&captured)
        .await
        .is_err());
    fixture
        .store
        .agents
        .delete_one(doc! {"_id":captured.id.as_str()})
        .await
        .unwrap();
    assert!(fixture
        .store
        .agent_log_prefix_with_tail(&captured)
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_prefix_proof_refuses_missing_corrupt_and_uncommitted_source() {
    for mode in ["legacy", "prepared"] {
        let fixture = fixture(mode).await;
        let mut captured = fixture.store.get_agent(&fixture.id).await.unwrap().unwrap();
        captured.conversation.last_seq = 200;
        for sequence in [1_i64, 130, 200, 260] {
            let filter = doc! {"agent_id":captured.id.as_str(),"seq":sequence};
            let row = fixture
                .store
                .agent_log
                .find_one(filter.clone())
                .await
                .unwrap()
                .unwrap();
            for corruption in ["missing", "identity", "sequence"] {
                match corruption {
                    "missing" => {
                        fixture
                            .store
                            .agent_log
                            .delete_one(filter.clone())
                            .await
                            .unwrap();
                    }
                    "identity" => {
                        fixture
                            .store
                            .agent_log
                            .update_one(filter.clone(), doc! {"$set":{"record.agent_id":"foreign"}})
                            .await
                            .unwrap();
                    }
                    "sequence" => {
                        fixture
                            .store
                            .agent_log
                            .update_one(filter.clone(), doc! {"$set":{"record.seq":sequence + 1}})
                            .await
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                fixture.reset_commands();
                assert!(
                    fixture
                        .store
                        .agent_log_prefix_with_tail(&captured)
                        .await
                        .is_err(),
                    "{mode}/{sequence}/{corruption}"
                );
                fixture.assert_read_only();
                fixture
                    .store
                    .agent_log
                    .replace_one(filter.clone(), row.clone())
                    .upsert(true)
                    .await
                    .unwrap();
            }
        }
        fixture
            .store
            .agent_log
            .insert_one(log_document(&entry(&captured.id, 261)).unwrap())
            .await
            .unwrap();
        fixture.reset_commands();
        assert!(fixture
            .store
            .agent_log_prefix_with_tail(&captured)
            .await
            .is_err());
        fixture.assert_read_only();
        fixture
            .store
            .agent_log
            .delete_one(doc! {"agent_id":captured.id.as_str(),"seq":261_i64})
            .await
            .unwrap();
        if mode == "prepared" {
            fixture
                .store
                .agents
                .update_one(
                    doc! {"_id":captured.id.as_str()},
                    doc! {"$set":{QUARANTINE_FIELD:{}}},
                )
                .await
                .unwrap();
            fixture.reset_commands();
            assert!(fixture
                .store
                .agent_log_prefix_with_tail(&captured)
                .await
                .is_err());
            fixture.assert_read_only();
        }
        fixture.cleanup().await;
    }
}
