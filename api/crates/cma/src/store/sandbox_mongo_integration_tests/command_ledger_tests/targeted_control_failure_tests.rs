use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_targeted_sidecar_audit_and_response_fence_failures_roll_back_every_write() {
    let (client, name) = database("targeted_rollback").await;
    let (store, _, request, _) = composed(&name, &client).await;
    for (collection, validator) in [
        (
            "agent_commands",
            doc! { "explicit_control": { "$exists": false } },
        ),
        (
            "command_authorization_audits",
            doc! { "forced_failure": true },
        ),
        (
            "agent_commands",
            doc! { "explicit_control_admission_fence": { "$exists": false } },
        ),
    ] {
        let before = snapshot(&store).await;
        client.database(&name).run_command(doc! { "collMod": collection, "validator": validator, "validationLevel": "strict", "validationAction": "error" }).await.unwrap();
        let result = store.reserve_targeted_control(request.clone()).await;
        client
            .database(&name)
            .run_command(doc! { "collMod": collection, "validator": {} })
            .await
            .unwrap();
        assert_eq!(result.unwrap_err(), CommandLedgerError::Unavailable);
        assert_eq!(snapshot(&store).await, before);
    }
    let admitted = store.reserve_targeted_control(request).await.unwrap();
    assert_eq!(admitted.record.receipt.agent_order, 2);
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn explicit_missing_stale_unbound_terminal_and_cross_owner_targets_refuse_atomically() {
    let (client, name) = database("targeted_refusal").await;
    let (store, _, request, source) = composed(&name, &client).await;
    let raw = store
        .agent_commands
        .find_one(doc! { "_id": source.receipt.envelope.command_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    for case in 0..7 {
        let mut changed = request.clone();
        let expected = match case {
            0 => {
                changed.target.response_id = AgentResponseId::generate();
                CommandLedgerError::NotFound
            }
            1 => {
                changed.target.turn_sequence += 1;
                CommandLedgerError::NotAdmissible
            }
            2 => {
                changed.command.expected_agent_revision += 1;
                CommandLedgerError::Conflict
            }
            3 => {
                changed.observed_agent_revision += 1;
                CommandLedgerError::Conflict
            }
            other => {
                let mut source = source.clone();
                match other {
                    4 => {
                        source.work.dispatch = None;
                        source.status = AgentResponseStatus::Queued;
                    }
                    5 => source.status = AgentResponseStatus::Completed,
                    _ => {
                        source.receipt.envelope.owner = AgentOwner::Person {
                            subject: SubjectId::new("foreign"),
                        }
                    }
                }
                let document = crate::store::agent_commands::command_document(&source).unwrap();
                store
                    .agent_commands
                    .replace_one(
                        doc! { "_id": source.receipt.envelope.command_id.as_str() },
                        document,
                    )
                    .await
                    .unwrap();
                if other == 6 {
                    CommandLedgerError::NotFound
                } else {
                    CommandLedgerError::NotAdmissible
                }
            }
        };
        let before = snapshot(&store).await;
        assert_eq!(
            store.reserve_targeted_control(changed).await.unwrap_err(),
            expected,
            "case {case}"
        );
        assert_eq!(snapshot(&store).await, before);
        store
            .agent_commands
            .replace_one(
                doc! { "_id": source.receipt.envelope.command_id.as_str() },
                raw.clone(),
            )
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn strict_targeted_sidecar_reader_rejects_malformed_or_contradictory_evidence() {
    let (client, name) = database("targeted_corrupt").await;
    let (store, _, request, _) = composed(&name, &client).await;
    let original = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    let id = original.binding.command_id.as_str();
    let raw = store
        .agent_commands
        .find_one(doc! { "_id": id })
        .await
        .unwrap()
        .unwrap();
    for fields in [
        doc! { "explicit_control.schema_version": 99_i64 },
        doc! { "explicit_control.unknown": true },
        doc! { "explicit_control.turn_sequence": 99_i64 },
        doc! { "explicit_control.command_id": "cmd_wrong" },
        doc! { "explicit_control.response_command_id": "cmd_missing" },
        doc! { "explicit_control.request_fingerprint": "a".repeat(64) },
        doc! { "explicit_control": "malformed" },
    ] {
        store
            .agent_commands
            .update_one(doc! { "_id": id }, doc! { "$set": fields })
            .await
            .unwrap();
        assert_eq!(
            store
                .lookup_targeted_control(request.clone())
                .await
                .unwrap_err(),
            CommandLedgerError::Corrupt
        );
        store
            .agent_commands
            .replace_one(doc! { "_id": id }, raw.clone())
            .await
            .unwrap();
    }
    store
        .agent_commands
        .update_one(
            doc! { "_id": id },
            doc! { "$unset": { "explicit_control": "" } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.lookup_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn targeted_unknown_commit_recovers_original_receipt_after_a_new_turn() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_targeted_commit_{suffix}");
    let app = format!("cma-targeted-commit-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let (store, agent, mut request, source) = composed(&name, &client).await;
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
        "data": { "failCommands": ["commitTransaction"], "appName": &app,
            "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected targeted commit ambiguity" } },
    }).await.unwrap();
    let result = store.reserve_targeted_control(request.clone()).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), CommandLedgerError::Unavailable);
    let record = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(record) = store
                .get_command(&request.command.envelope.command_id)
                .await
                .unwrap()
            {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("committed target becomes majority visible");
    let current = finish_and_start(&store, &agent, &source).await;
    request.observed_agent_revision = current.revision;
    let recovered = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    assert_eq!(recovered.record, record);
    assert_eq!(recovered.binding.target(), request.target);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(current));
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! { "explicit_control": { "$exists": true } })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
