//! A committed edit remains recoverable after its original AM loses ownership.
use super::*;

async fn database_snapshot(database: &mongodb::Database) -> Vec<(String, Vec<Document>)> {
    let mut names = database.list_collection_names().await.unwrap();
    names.sort();
    let mut rows = Vec::new();
    for name in names {
        let documents = database
            .collection::<Document>(&name)
            .find(doc! {})
            .sort(doc! {"_id": 1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        rows.push((name, documents));
    }
    rows
}

fn edit_request(
    agent: &Agent,
    parent: &Sandbox,
    source: &AdmittedAgentCommand,
) -> QueuedInputReservation {
    let mut command = live_request(agent, parent, "retained-edit-key");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    QueuedInputReservation {
        cancellation: QueuedCancellationReservation {
            command,
            target: QueuedCancellationTarget {
                response_id: source.receipt.response_id.clone().unwrap(),
                source_command_id: source.receipt.envelope.command_id.clone(),
                expected_command_revision: source.revision,
            },
            observed_agent_revision: agent.revision,
            agent_created_at: agent.created_at.clone(),
        },
        text: "\u{1}".repeat(layer2_core::sandbox::MAX_AGENT_MESSAGE_BYTES),
    }
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_URI"]
async fn committed_edit_lookup_after_owner_handoff_is_exact_and_never_writes() {
    let (client, name, raw) = database().await;
    let (parent, agent) = fixture(&raw, "edit-recovery").await;
    let source = record(
        raw.reserve_command(live_request(&agent, &parent, "queued-source"))
            .await
            .unwrap(),
    );
    let request = edit_request(&agent, &parent, &source);
    request.validate().unwrap();
    // Finish opening stores and indexes before taking the short owner lease.
    let store = raw
        .enable_agent_ownership("admission-owner".into())
        .await
        .unwrap();
    let successor = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .enable_agent_ownership("successor-owner".into())
        .await
        .unwrap();
    let authority = owner(&store, &agent.id).await;
    let QueuedInputOutcome::Applied(committed) = authority
        .run(store.edit_queued_input(request.clone()))
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("fresh queued edit must commit");
    };
    assert_eq!(committed.record.receipt.agent_order, 2);
    assert_eq!(committed.response.revision, source.revision + 1);
    assert_eq!(committed.binding.text, request.text);

    store
        .agent_ownership
        .as_ref()
        .unwrap()
        .heartbeat("admission-owner", true)
        .await
        .unwrap();
    store
        .agent_ownership
        .as_ref()
        .unwrap()
        .release(&authority.token)
        .await
        .unwrap();
    let coordinator = OwnershipCoordinator::register(
        successor.agent_ownership.clone().unwrap(),
        Member {
            instance_id: "successor-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:2".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 32,
        },
    )
    .await
    .unwrap();
    let next = coordinator.acquire(&agent.id).await.unwrap().unwrap();
    assert!(next.token.epoch > authority.token.epoch);
    assert_ne!(next.token.instance_id, authority.token.instance_id);
    assert_eq!(
        authority
            .run(store.edit_queued_input(request.clone()))
            .await
            .unwrap()
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(
        next.run(successor.edit_queued_input(request.clone()))
            .await
            .unwrap()
            .unwrap(),
        QueuedInputOutcome::Applied(committed.clone()),
        "same-key replay on the new owner must retain the exact receipt"
    );

    let db = client.database(&name);
    let before = database_snapshot(&db).await;
    assert!(layer2_core::agent_ownership::execution_authority().is_none());
    assert_eq!(
        store.lookup_queued_input(request.clone()).await.unwrap(),
        Some(*committed.clone()),
        "the former owner can recover a receipt without acquiring authority"
    );
    assert_eq!(
        successor
            .lookup_queued_input(request.clone())
            .await
            .unwrap(),
        Some(*committed.clone())
    );
    let mut changed_text = request.clone();
    changed_text.text = "changed body under the original key".into();
    let mut changed_target = request.clone();
    changed_target.cancellation.target.expected_command_revision += 1;
    for changed in [changed_text, changed_target] {
        assert_eq!(
            store.lookup_queued_input(changed).await.unwrap_err(),
            CommandLedgerError::IdempotencyConflict
        );
    }
    let mut absent = request.clone();
    absent.cancellation.command.key_sha256 = command_key_digest("unknown-edit-key").unwrap();
    assert!(store.lookup_queued_input(absent).await.unwrap().is_none());
    let mut expired = request;
    expired.cancellation.command.authority_deadline = Some(tokio::time::Instant::now());
    assert_eq!(
        store.lookup_queued_input(expired).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(
        database_snapshot(&db).await,
        before,
        "lookup must not write resource, ledger, audit, membership or owner rows"
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    assert_eq!(
        store
            .get_command(&source.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        source.revision + 1
    );
    db.drop().await.unwrap();
}
