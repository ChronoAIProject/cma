//! Active capacity checks use bounded indexed counts only when configured.
use super::*;
use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsState, ExistingReserve};

pub(super) async fn configure(store: &MongoSandboxStore, ceilings: AdmissionCeilings) {
    let row = store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .await
        .unwrap()
        .unwrap();
    let state = crate::store::admission_ceilings::from_document(&row).unwrap();
    let next = state
        .changed(
            state.revision,
            &SubjectId::new("operator"),
            "configure active capacity",
            ceilings,
            100,
        )
        .unwrap()
        .unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": mongodb::bson::serialize_to_bson(&next).unwrap()}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn idle_agent_refusal_preserves_public_reserve_and_per_owner_capacity() {
    let (client, name) = database("session_precheck").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    store.agents.insert_one(doc! {"_id": "active-other", "owner": agent.owner.as_str(), "status": "waiting_approval"}).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            active_sessions: Some(2),
            existing_reserve: ExistingReserve {
                active_sessions: Some(1),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await;
    store
        .sandbox_admissions
        .insert_one(doc! {
            "_id": {"scope": "registration", "subject": agent.owner.as_str()},
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "registered_at": 100_i64,
        })
        .await
        .unwrap();
    let refused = request(&agent, &parent, "public-refusal");
    assert_eq!(
        store.reserve_command(refused).await.unwrap_err(),
        CommandLedgerError::SessionCapacityUnavailable
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    store
        .sandbox_admissions
        .delete_one(doc! {"_id": {"scope": "registration", "subject": agent.owner.as_str()}})
        .await
        .unwrap();
    let accepted = request(&agent, &parent, "existing-reserve");
    let receipt = store.reserve_command(accepted.clone()).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            owner_active_sessions: Some(1),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "owner-refusal"))
            .await
            .unwrap_err(),
        CommandLedgerError::SessionCapacityUnavailable
    );
    assert_eq!(
        record(store.reserve_command(accepted).await.unwrap()),
        record(receipt)
    );
    assert_eq!(
        store
            .agents
            .count_documents(doc! {"status": "waiting_approval"})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unreadable_ceilings_refuse_fresh_idle_messages_without_durable_admission() {
    let (client, name) = database("session_unreadable").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": null}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "unreadable"))
            .await
            .unwrap_err(),
        CommandLedgerError::SessionCapacityUnavailable
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_turn_starts_serialize_the_last_slot_and_publish_status_with_the_log() {
    use super::work_tests::{claim_request, live_request, message_mutation};
    let (client, name) = database("session_start_boundary").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            active_sessions: Some(1),
            ..Default::default()
        },
    )
    .await;
    let mut mutations = Vec::new();
    for index in 0..12 {
        let mut parent = active_sandbox(&format!("sbx_session_boundary_{index}"));
        parent.owner = SubjectId::new(format!("session-boundary-owner-{index}"));
        store.create_sandbox(parent.clone()).await.unwrap();
        let agent = ready_agent(&parent, &format!("agt_session_boundary_{index}"));
        insert_agent(&store, &agent, NOW).await;
        store
            .reserve_command(live_request(&agent, &parent, &format!("boundary-{index}")))
            .await
            .unwrap();
        let command = store
            .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
            .await
            .unwrap()
            .unwrap();
        mutations.push(message_mutation(
            &agent,
            &parent,
            &command.work.lease.unwrap().claim,
        ));
    }
    // Distinct owners and parents isolate the fleet fence from per-parent
    // contention. Durable queued commands do not count as active Agents, so
    // the start fence is authoritative after all prechecks saw an empty fleet.
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! {"status": "queued"})
            .await
            .unwrap(),
        12
    );
    assert_eq!(
        store
            .agents
            .count_documents(doc! {"status": "working"})
            .await
            .unwrap(),
        0
    );
    let second = MongoSandboxStore::open(&client, &name).await.unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(mutations.len()));
    let results = futures_util::future::join_all(mutations.into_iter().enumerate().map(
        |(index, mutation)| {
            let handle = if index % 2 == 0 {
                store.clone()
            } else {
                second.clone()
            };
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                // execute_message retries a conflicted start publication before
                // dispatch. Keep the same mutation and claim, and use its eight
                // operation attempts as a strict bound for this store-level test.
                let mut conflicts = 0;
                for attempt in 1..=8 {
                    let result = handle.commit_agent_conversation(mutation.clone()).await;
                    if matches!(result, Err(SandboxStoreError::Conflict)) {
                        assert!(attempt < 8, "start publication exhausted conflict retries");
                        conflicts += 1;
                        continue;
                    }
                    return (result, conflicts, attempt);
                }
                unreachable!("the last conflict fails instead of becoming a final outcome")
            }
        },
    ))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|(result, _, _)| result.is_ok())
            .count(),
        1
    );
    let retried_conflicts: usize = results.iter().map(|(_, conflicts, _)| conflicts).sum();
    let publication_attempts: usize = results.iter().map(|(_, _, attempts)| attempts).sum();
    assert_eq!(publication_attempts, 12 + retried_conflicts);
    for (result, conflicts, attempts) in results {
        assert_eq!(attempts, conflicts + 1);
        if let Err(error) = result {
            assert_eq!(error, SandboxStoreError::SessionCapacityUnavailable);
        }
    }
    assert_eq!(
        store
            .agents
            .count_documents(doc! {"status": "working"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"record.kind.type": "turn_started"})
            .await
            .unwrap(),
        1
    );
    eprintln!(
        "active_sessions N=12 limit=1 admitted_commands=12 started=1 refused_starts=11 overshoot=0 retried_conflicts={retried_conflicts} publication_attempts={publication_attempts} final_conflicts=0"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn per_owner_capacity_is_rechecked_when_an_admitted_command_starts() {
    use super::work_tests::{claim_request, live_request, message_mutation};
    let (client, name) = database("session_owner_start").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(
        &store,
        AdmissionCeilings {
            owner_active_sessions: Some(1),
            ..Default::default()
        },
    )
    .await;
    store
        .reserve_command(live_request(&agent, &parent, "admitted-owner"))
        .await
        .unwrap();
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    store
        .agents
        .insert_one(
            doc! {"_id": "owner-now-active", "owner": agent.owner.as_str(), "status": "working"},
        )
        .await
        .unwrap();
    let mutation = message_mutation(&agent, &parent, &claimed.work.lease.unwrap().claim);
    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        SandboxStoreError::SessionCapacityUnavailable
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn session_count_failure_rolls_back_admission_and_unset_never_requires_the_index() {
    let (client, name) = database("session_count_failure").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure(
        &store,
        AdmissionCeilings {
            active_sessions: Some(1),
            ..Default::default()
        },
    )
    .await;
    store
        .agents
        .drop_index("agents_live_connections")
        .await
        .unwrap();
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "count-failed"))
            .await
            .unwrap_err(),
        CommandLedgerError::Unavailable
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    configure(&store, AdmissionCeilings::default()).await;
    store
        .reserve_command(request(&agent, &parent, "unset-without-index"))
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unset_ceilings_add_zero_mongo_commands_to_fresh_messages() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!(
        "cma_session_zero_commands_{}",
        uuid::Uuid::new_v4().simple()
    );
    let (store, agent, parent) = fixture(&name, &client).await;
    let mut prime = request(&agent, &parent, "prime");
    prime.queue_capacity = 10;
    store.reserve_command(prime).await.unwrap();
    trace.lock().unwrap().clear();
    let mut baseline = request(&agent, &parent, "missing");
    baseline.queue_capacity = 10;
    store.reserve_command(baseline).await.unwrap();
    let missing = trace.lock().unwrap().clone();
    store.budget_settings.update_one(doc! {"_id": "platform"}, doc! {"$set": {"admission_ceilings": mongodb::bson::serialize_to_bson(&AdmissionCeilingsState::default()).unwrap()}}).await.unwrap();
    trace.lock().unwrap().clear();
    let mut explicit = request(&agent, &parent, "explicit");
    explicit.queue_capacity = 10;
    store.reserve_command(explicit).await.unwrap();
    let unset = trace.lock().unwrap().clone();
    assert_eq!(
        missing.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        unset.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(!unset.iter().any(
        |(_, command)| command.get_str("aggregate").ok() == Some("agents")
            || command.get_str("find").ok() == Some("sandbox_admissions")
    ));
    assert_eq!(
        unset
            .iter()
            .filter(|(_, command)| command.get_str("find").ok() == Some("platform_settings"))
            .count(),
        1
    );
    assert_eq!(
        unset
            .iter()
            .filter(|(_, command)| command.get_str("update").ok() == Some("platform_settings"))
            .count(),
        1
    );
    eprintln!("active_sessions unset commands={} extra_commands=0 settings_reads=1 settings_fences=1 capacity_counts=0 cohort_reads=0", unset.len());
    client.database(&name).drop().await.unwrap();
}
