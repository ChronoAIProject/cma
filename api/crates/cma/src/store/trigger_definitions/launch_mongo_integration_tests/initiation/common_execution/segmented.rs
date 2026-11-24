use super::*;
use crate::store::agent_log_segments::{
    command_worker, publication, schema, terminal, transaction_scope::TransactionScope, writer_gate,
};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

async fn append(f: &Fixture, agent: &Agent, key: &str) {
    let kind = AgentLogKind::ModeChanged {
        mode: "code".into(),
    };
    f.resources
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: key.into(),
                fingerprint: publication::request_digest(&kind)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
                appended_at: agent.created_at.clone(),
            },
            kind,
        )
        .await
        .unwrap();
}

async fn adopt(f: &Fixture, agent: &Agent) {
    let db = f.seed.client.database(&f.seed.database);
    schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let mut session = f.seed.client.start_session().await.unwrap();
    for (revision, enabled) in [(0, true), (1, false)] {
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        if let Err(error) = writer_gate::replace(
            &db,
            writer_gate::GateName::Segments,
            revision,
            writer_gate::Policy {
                enabled,
                agents: if enabled {
                    vec![agent.id.to_string()]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            &mut session,
        )
        .await
        {
            panic!("gate: {error:?}")
        }
        session.commit_transaction().await.unwrap();
        if enabled {
            append(f, agent, "adopt-trigger").await;
        }
    }
}

pub(super) async fn accepted() -> Accepted {
    let (f, launch, request) = pending().await;
    let agent = f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    let binding = agent
        .conversation
        .first_message
        .as_ref()
        .unwrap()
        .common_binding
        .as_ref()
        .unwrap();
    let command = f
        .resources
        .get_command(&binding.receipt().envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    let launch = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    adopt(&f, &agent).await;
    let agent = f.resources.get_agent(&agent.id).await.unwrap().unwrap();
    Accepted {
        f,
        launch,
        request,
        agent,
        command,
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_trigger_terminal_roots_preserve_original_command_and_emit_no_input() {
    for claimed in [false, true] {
        let a = accepted().await;
        let now = a.command.queue_deadline.clone();
        if claimed {
            let at = layer2_core::seconds_from_rfc3339(&now).unwrap();
            let command =
                a.f.resources
                    .claim_next_command(CommandClaimRequest {
                        agent_id: a.agent.id.clone(),
                        holder: "expiry-worker".into(),
                        now: now.clone(),
                        expires_at: layer2_core::rfc3339_at(
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(at as u64 + 60),
                        ),
                    })
                    .await
                    .unwrap()
                    .unwrap();
            a.f.resources
                .reject_command(
                    &command.work.lease.unwrap().claim,
                    CommandFailureCode::QueueTimeout,
                    now,
                )
                .await
                .unwrap();
        } else {
            a.f.resources
                .reject_unclaimed_command(
                    &a.command.receipt.envelope.command_id,
                    a.command.revision,
                    CommandFailureCode::QueueTimeout,
                    now,
                )
                .await
                .unwrap();
        }
        let agent = a.current_agent().await;
        assert_eq!(
            agent.conversation.last_seq,
            a.agent.conversation.last_seq + 1
        );
        assert_eq!(
            agent.conversation.first_message.as_ref().unwrap().state,
            FirstMessageDelivery::Failed
        );
        assert!(agent.conversation.operation.is_none() && agent.conversation.active_turn.is_none());
        let command = a.current_command().await;
        assert_eq!(command.receipt, a.command.receipt);
        assert!(command.work.dispatch.is_none());
        let rows = a.f.resources.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(
            rows.iter()
                .filter(|entry| matches!(
                    entry.kind,
                    AgentLogKind::FirstMessageChanged {
                        state: FirstMessageDelivery::Failed,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(!rows.iter().any(|entry| matches!(
            entry.kind,
            AgentLogKind::OwnerMessage { .. } | AgentLogKind::TurnStarted { .. }
        )));
        a.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_trigger_refusal_aborts_command_on_missing_source_evidence() {
    let a = accepted().await;
    let before = a.current_agent().await;
    a.f.seed
        .client
        .database(&a.f.seed.database)
        .collection::<Document>("agent_log_heads")
        .delete_one(doc! {"_id":a.agent.id.as_str()})
        .await
        .unwrap();
    assert!(a
        .f
        .resources
        .reject_unclaimed_command(
            &a.command.receipt.envelope.command_id,
            a.command.revision,
            CommandFailureCode::QueueTimeout,
            a.command.queue_deadline.clone()
        )
        .await
        .is_err());
    assert_eq!(a.current_agent().await, before);
    assert_eq!(
        a.f.resources
            .get_command(&a.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        a.command
    );
    a.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_trigger_exact_refusal_receipt_survives_later_append() {
    let a = accepted().await;
    let parent = doc! { "_id": a.agent.sandbox_id.as_str() };
    a.f.resources
        .sandboxes
        .update_one(
            parent.clone(),
            doc! { "$set": { "ledger_maintenance": { "phase": "requested" } } },
        )
        .await
        .unwrap();
    let before = a.current_agent().await;
    assert_eq!(
        a.f.resources
            .reject_unclaimed_command(
                &a.command.receipt.envelope.command_id,
                a.command.revision,
                CommandFailureCode::QueueTimeout,
                a.command.queue_deadline.clone(),
            )
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(a.current_agent().await, before);
    assert_eq!(
        a.f.resources
            .get_command(&a.command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(a.command.clone())
    );
    a.f.resources
        .sandboxes
        .update_one(parent, doc! { "$unset": { "ledger_maintenance": "" } })
        .await
        .unwrap();
    let mut session = a.f.seed.client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = command_worker::refuse(
        &a.f.resources,
        &a.command,
        a.command.revision,
        CommandFailureCode::QueueTimeout,
        a.command.queue_deadline.clone(),
        &mut scope,
    )
    .await
    .unwrap()
    .unwrap();
    scope.release().commit_transaction().await.unwrap();
    append(&a.f, &a.agent, "later-trigger-append").await;
    a.f.resources
        .agents
        .find_one(doc! {"_id":a.agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    assert!(
        terminal::reconcile(&a.f.resources, Some(&receipt), session.operation_time())
            .await
            .unwrap()
    );
    a.f.seed
        .client
        .database(&a.f.seed.database)
        .collection::<Document>("agent_log_publications")
        .delete_one(doc! {"publication_key":{"$regex":"^trigger-terminal:"}})
        .await
        .unwrap();
    a.f.resources
        .agents
        .find_one(doc! {"_id":a.agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    assert!(
        !terminal::reconcile(&a.f.resources, Some(&receipt), session.operation_time())
            .await
            .unwrap()
    );
    a.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_prepared_trigger_reuse_keeps_original_binding_and_format() {
    let (f, launch, request) = pending().await;
    let original = reuse::prepared(&f, &request).await;
    adopt(&f, &original).await;
    let result = f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await
        .unwrap();
    let agent = &result.creation().agent;
    assert_eq!(agent.id, original.id);
    let first = agent.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.origin_key, original.creation_key);
    let binding = first.common_binding.as_ref().unwrap();
    assert_eq!(binding.receipt().envelope.agent_id, original.id);
    append(&f, agent, "later-reused-trigger").await;
    let replay = f
        .resources
        .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(replay.creation().agent.id, original.id);
    assert_eq!(
        replay
            .creation()
            .agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding
            .as_ref(),
        Some(binding)
    );
    f.close().await;
}

mod unknown;

mod cancellation;
