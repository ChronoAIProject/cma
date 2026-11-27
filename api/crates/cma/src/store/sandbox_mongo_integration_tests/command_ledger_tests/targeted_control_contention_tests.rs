use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Release,
    Persistent,
    Expired,
    AgentChanged,
    ParentChanged,
}

async fn contended(case: Case, inspect: bool) {
    let (client, name) = database("targeted_contention").await;
    let (store, agent, mut request, _) = composed(&name, &client).await;
    let before = snapshot(&store).await;
    let conflicts = Arc::new(Mutex::new((
        BTreeMap::<i32, String>::new(),
        BTreeSet::<String>::new(),
    )));
    let observed = conflicts.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut state = observed.lock().unwrap();
        match event {
            CommandEvent::Started(event)
                if matches!(event.command.get_bool("autocommit"), Ok(false)) =>
            {
                state.0.insert(
                    event.request_id,
                    format!(
                        "{:?}:{:?}",
                        event.command.get("lsid"),
                        event.command.get("txnNumber")
                    ),
                );
            }
            CommandEvent::Failed(event) => {
                if let Some(identity) = state.0.remove(&event.request_id) {
                    if is_transient(&event.failure) {
                        state.1.insert(identity);
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                state.0.remove(&event.request_id);
            }
            _ => {}
        }
    }));
    let replica = Client::with_options(options).unwrap();
    let contender = MongoSandboxStore::open(&replica, &name).await.unwrap();
    let mut holder = client.start_session().await.unwrap();
    start_transaction(&mut holder).await.unwrap();
    // A real competing transaction locks the same resource row without changing
    // its logical revision, as admission fences and quiet message checkpoints do.
    let (collection, id) = if case == Case::ParentChanged {
        (&store.sandboxes, agent.sandbox_id.as_str())
    } else {
        (&store.agents, agent.id.as_str())
    };
    let mut increments = doc! { "agent_command_admission_fence": 1_i64 };
    if matches!(case, Case::AgentChanged | Case::ParentChanged) {
        increments.insert("revision", 1_i64);
        increments.insert("record.revision", 1_i64);
    }
    collection
        .update_one(doc! { "_id": id }, doc! { "$inc": increments })
        .session(&mut holder)
        .await
        .unwrap();
    if case == Case::Expired {
        request.command.authority_deadline =
            Some(tokio::time::Instant::now() + Duration::from_millis(200));
    }
    let retry = request.clone();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move {
        if inspect {
            match contender.inspect_targeted_control(request).await? {
                TargetedControlInspection::Missing => Ok(None),
                _ => panic!("unsubmitted current intent must remain missing"),
            }
        } else {
            contender.reserve_targeted_control(request).await.map(Some)
        }
    });
    let release = !matches!(case, Case::Persistent | Case::Expired);
    if release {
        let observed = tokio::time::timeout(Duration::from_secs(4), async {
            while conflicts.lock().unwrap().1.len() < 6 && !task.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        holder.commit_transaction().await.unwrap();
        observed.unwrap();
    }
    let result = tokio::time::timeout(Duration::from_secs(9), task).await;
    if !release {
        holder.commit_transaction().await.unwrap();
    }
    let result = result.unwrap().unwrap();
    let attempts = conflicts.lock().unwrap().1.len();
    if case != Case::Expired {
        assert!(attempts >= 6, "observed only {attempts} genuine conflicts");
    }
    if case == Case::Release {
        if inspect {
            assert_eq!(result, Ok(None));
            assert_eq!(&snapshot(&store).await[2..], &before[2..]);
            assert_eq!(
                store.agent_commands.count_documents(doc! {}).await.unwrap(),
                1
            );
        } else {
            let accepted = result.unwrap().unwrap();
            assert_eq!(accepted.binding.target(), retry.target);
            assert_eq!(
                accepted.record.receipt.envelope.command_id,
                retry.command.envelope.command_id
            );
            assert_eq!(
                store.reserve_targeted_control(retry).await.unwrap(),
                accepted
            );
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
        }
    } else {
        let expected = if case == Case::Persistent {
            CommandLedgerError::ControlBusy
        } else {
            CommandLedgerError::Conflict
        };
        assert_eq!(result, Err(expected));
        // A failed transaction must leave source/sidecar documents, queue order,
        // and authorization audits byte-for-byte unchanged.
        assert_eq!(&snapshot(&store).await[2..], &before[2..]);
        assert_eq!(
            store.agent_commands.count_documents(doc! {}).await.unwrap(),
            1
        );
        assert_eq!(
            store
                .command_authorization_audits
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        if case == Case::Persistent {
            assert!(started.elapsed() >= Duration::from_secs(4));
            assert!(started.elapsed() < Duration::from_secs(9));
            let accepted = store.reserve_targeted_control(retry).await.unwrap();
            assert_eq!(accepted.record.receipt.agent_order, 2);
            assert_eq!(
                store
                    .agent_commands
                    .count_documents(doc! { "explicit_control": { "$exists": true } })
                    .await
                    .unwrap(),
                1
            );
        } else if case == Case::Expired {
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(
                store.reserve_targeted_control(retry).await,
                Err(CommandLedgerError::Conflict)
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_survives_six_real_write_conflicts_and_replays_once() {
    contended(Case::Release, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_inspection_survives_six_real_write_conflicts() {
    contended(Case::Release, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_persistent_contention_is_bounded_and_exact_retry_recovers() {
    contended(Case::Persistent, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_contention_never_extends_authority_or_retargets_changed_resources() {
    for case in [Case::Expired, Case::AgentChanged, Case::ParentChanged] {
        contended(case, false).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn coordinated_response_control_waits_for_local_writer_and_commits_once() {
    let (client, name) = database("targeted_local_coordination").await;
    let (store, _agent, request, _) = composed(&name, &client).await;
    let store = store.with_agent_control_write_coordination(true);
    let writer = store.agent_writes.lock(&_agent.id).await;
    let task_store = store.clone();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move { task_store.reserve_targeted_control(request).await });

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        !task.is_finished(),
        "control must wait behind a local writer"
    );
    drop(writer);

    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("local coordination must preserve the Stop budget")
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed();
    println!(
        "coordinated response control latency_ms={}",
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(result.record.receipt.agent_order, 2);
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! { "explicit_control": { "$exists": true } })
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
