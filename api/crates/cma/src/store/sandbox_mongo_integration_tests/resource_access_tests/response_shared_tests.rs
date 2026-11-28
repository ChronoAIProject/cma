//! Paired complete authenticated routes, including relay and read custody.
use super::*;
use serde_json::Value;
#[path = "response_shared_auth_errors.rs"]
mod auth_errors;
#[path = "response_shared_generation.rs"]
mod generation;
#[path = "response_shared_limits.rs"]
mod limits;
#[path = "response_open_attempt.rs"]
mod open_attempt;
#[path = "response_shared_readiness.rs"]
mod readiness;
#[path = "response_shared_release.rs"]
mod release;
#[path = "response_shared_trace.rs"]
mod trace;

async fn retained(rows: u64) -> (Fixture, layer2_core::ids::AgentResponseId) {
    let mut f = Fixture::semantic(rows).await;
    let turn = rows - 9;
    for (seq, kind) in [
        (turn - 1, AgentLogKind::TurnCompleted { turn_sequence: 3 }),
        (
            turn,
            AgentLogKind::TurnStarted {
                turn_sequence: turn,
                model: Some("retained-model".into()),
                origin_key: None,
            },
        ),
        (
            turn + 1,
            AgentLogKind::OwnerMessage {
                content: "retained question".into(),
            },
        ),
        (
            turn + 2,
            AgentLogKind::AgentMessage {
                content: "retained answer".into(),
            },
        ),
    ] {
        let row = AgentLogEntry {
            agent_id: f.agent.id.clone(),
            seq,
            appended_at: NOW.into(),
            kind,
        };
        f.store
            .agent_log
            .replace_one(
                doc! {"agent_id":f.agent.id.as_str(),"seq":seq as i64},
                log_document(&row).unwrap(),
            )
            .await
            .unwrap();
    }
    f.agent.conversation.active_turn = Some(turn);
    f.agent.revision += 1;
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    let row = f
        .store
        .agent_commands
        .find_one(doc! {"response_id":{"$type":"string"}})
        .await
        .unwrap()
        .unwrap();
    let mut command = crate::store::agent_commands::records::read(&row).unwrap();
    let dispatch = command.work.dispatch.as_mut().unwrap();
    dispatch.turn_sequence = turn;
    dispatch.operation_sequence = turn;
    let response = command.receipt.response_id.clone().unwrap();
    f.store
        .agent_commands
        .replace_one(
            doc! {"_id":command.receipt.envelope.command_id.as_str()},
            crate::store::agent_commands::records::document(&command).unwrap(),
        )
        .await
        .unwrap();
    f.store = f
        .store
        .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
        .await
        .unwrap();
    (f, response)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
fn request(
    client: &reqwest::Client,
    f: &Fixture,
    server: &Server,
    path: &str,
) -> reqwest::RequestBuilder {
    client
        .get(format!("{}{path}", server.address))
        .bearer_auth(format!("observer:{}", f.agent.owner))
}
fn events(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|json| serde_json::from_str(json).unwrap())
        .collect()
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_response_snapshot_remains_available_during_observer_handoff() {
    let (f, id) = retained(100).await;
    let replicas = servers(&f).await;
    let initial_slots: Vec<_> = replicas
        .iter()
        .map(|replica| replica.relay.available_test_slots())
        .collect();
    let destination = replicas[0]
        .observation
        .destination(&f.agent.id, &f.agent.created_at)
        .await
        .unwrap();
    let owner = replicas
        .iter()
        .position(|replica| replica.address == destination.member.relay_address)
        .unwrap();
    let peer = &replicas[1 - owner];
    let client = client();
    let path = format!("/api/v2/responses/{id}");
    let response = request(&client, &f, peer, &path).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let before: Value = response.json().await.unwrap();
    assert_eq!(before["id"], id.as_str());
    assert_eq!(before["conversation"]["id"], f.agent.id.as_str());
    assert_eq!(before["status"], "in_progress");

    // The observation lease still targets the previous replica when its
    // listener stops answering. This must not make the durable receipt vanish.
    replicas[owner]
        .stall
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let response = request(&client, &f, peer, &path).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap(), before);

    let denied = client
        .get(format!("{}{path}", peer.address))
        .bearer_auth("observer:unrelated-subject")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(!denied.text().await.unwrap().contains("retained answer"));
    observation_capacity::available(&replicas, &initial_slots).await;
    drop(replicas);
    f.client.database(&f.name).drop().await.unwrap();
}

async fn measure(
    f: &Fixture,
    id: &layer2_core::ids::AgentResponseId,
    enabled: bool,
    observers: usize,
) -> (meter::Stats, Vec<Vec<Value>>) {
    let (start_idle, idle_start) = tokio::sync::watch::channel(false);
    let trace_open =
        std::env::var(crate::config::OBSERVATION_OPEN_TRACE_ENABLED).as_deref() == Ok("true");
    let replicas = response_budget_servers(f, enabled, idle_start, trace_open).await;
    let client = client();
    f.meter.clear();
    let path = format!("/api/v2/responses/{id}/events");
    let open = |index: usize| {
        let (client, replica, path) = (&client, &replicas[index % 2], &path);
        async move {
            let request = request(client, f, replica, path);
            let response = if trace_open {
                open_attempt::send(request, replica, enabled, index).await
            } else {
                request.send().await.unwrap()
            };
            assert_eq!(
                response.status(),
                reqwest::StatusCode::OK,
                "{}",
                response.text().await.unwrap()
            );
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let captured = buffer.clone();
            let opened = std::time::Instant::now();
            let consumer = tokio::spawn(async move {
                let mut body = response.bytes_stream();
                while let Some(chunk) = body.next().await {
                    captured.lock().unwrap().extend_from_slice(&chunk.unwrap());
                }
                let captured = captured.lock().unwrap();
                let last_event = events(&captured).pop();
                let kind = last_event.as_ref().and_then(|event| event["type"].as_str());
                let code = last_event.as_ref().and_then(|event| event["code"].as_str());
                panic!(
                    "unchanged response stream closed: shared={enabled} observer={index} age={:?} last_type={kind:?} code={code:?}",
                    opened.elapsed()
                );
            });
            (buffer, consumer)
        }
    };
    // Headers can arrive before replay. Bound complete cold opens equally on
    // both routes and let each cohort finish replay before opening the next.
    let mut buffers = Vec::with_capacity(observers);
    let mut consumers = Vec::with_capacity(observers);
    for start in (0..observers).step_by(8) {
        let (opened, readers): (Vec<_>, Vec<_>) =
            futures_util::future::join_all((start..(start + 8).min(observers)).map(open))
                .await
                .into_iter()
                .unzip();
        readiness::wait_for_replay(&opened, &readers).await;
        buffers.extend(opened);
        consumers.extend(readers);
    }
    let cold_snapshots = f.meter.stats.lock().unwrap().snapshot_commands;
    start_idle.send_replace(true);
    tokio::time::sleep(Duration::from_secs(12)).await;
    let measured = f.meter.stats.lock().unwrap().clone();
    eprintln!(
        "response_snapshot_commands shared={enabled} observers={observers} cold={cold_snapshots} total={}",
        measured.snapshot_commands
    );
    for consumer in consumers {
        assert!(!consumer.is_finished());
        consumer.abort();
        let _ = consumer.await;
    }
    let bodies = buffers
        .iter()
        .map(|buffer| events(&buffer.lock().unwrap()))
        .collect();
    drop(replicas);
    (measured, bodies)
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_response_complete_routes_reduce_unchanged_decode_and_snapshots_with_exact_replay() {
    let (f, id) = retained(1_000).await;
    compare_unchanged(f, id, "legacy", 8).await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_adopted_response_complete_routes_meet_unchanged_budgets_and_exact_replay() {
    let (f, id) = generation::adopted().await;
    // The closed adopted route already avoids idle source rescans. Include
    // cold opening across the plan's 100-observer cohort in both modes.
    compare_unchanged(f, id, "segmented", 100).await;
}

async fn compare_unchanged(
    f: Fixture,
    id: layer2_core::ids::AgentResponseId,
    format: &str,
    observers: usize,
) {
    let (closed, old) = measure(&f, &id, false, observers).await;
    let (shared, new) = measure(&f, &id, true, observers).await;
    assert_eq!(
        old, new,
        "every response object and replay sequence must match"
    );
    assert!(old.iter().all(|events| events.len() >= 8));
    assert!(
        shared.history_bytes * 10 <= closed.history_bytes,
        "history bytes {} -> {}",
        closed.history_bytes,
        shared.history_bytes
    );
    assert!(
        shared.snapshot_commands * 4 <= closed.snapshot_commands,
        "snapshots {} -> {}",
        closed.snapshot_commands,
        shared.snapshot_commands
    );
    eprintln!("response_complete_route format={format} observers={observers} replicas=2 seconds=12 closed_history_bson={} shared_history_bson={} closed_snapshot_commands={} shared_snapshot_commands={} closed_commands={} shared_commands={}",
        closed.history_bytes,shared.history_bytes,closed.snapshot_commands,shared.snapshot_commands,
        closed.commands.values().sum::<u64>(),shared.commands.values().sum::<u64>());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_response_get_replay_and_cursor_suffix_match_the_closed_route() {
    let (f, id) = retained(1_000).await;
    let client = client();
    let closed = servers_mode(&f, false).await;
    let path = format!("/api/v2/responses/{id}");
    let expected = request(&client, &f, &closed[0], &path)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(expected["model"], "retained-model");
    drop(closed);
    let replicas = servers_mode(&f, true).await;
    for server in &replicas {
        let value = request(&client, &f, server, &path)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(value, expected);
    }
    let response = request(
        &client,
        &f,
        &replicas[1],
        &format!("{path}/events?starting_after=3"),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut body = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(events(&first)[0]["sequence_number"], 4);
    assert!(String::from_utf8_lossy(&first).contains("id: 4"));
    drop(body);
    drop(replicas);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_response_command_only_rejection_preserves_the_log_and_exact_terminal_evidence() {
    let (f, _) = retained(1_000).await;
    let parent = f
        .store
        .get_sandbox(&f.agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let reservation = crate::store::sandbox_mongo_integration_tests::command_ledger_tests::request(
        &f.agent,
        &parent,
        "shared-queued",
    );
    let record = reservation
        .record(3, f.agent.conversation.last_seq, None)
        .unwrap();
    let id = record.receipt.response_id.as_ref().unwrap();
    f.store
        .agent_commands
        .insert_one(crate::store::agent_commands::records::document(&record).unwrap())
        .await
        .unwrap();
    let replicas = servers_mode(&f, true).await;
    let response = request(
        &client(),
        &f,
        &replicas[1],
        &format!("/api/v2/responses/{id}/events"),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut body = response.bytes_stream();
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(events(&first)[0]["type"], "response.created");
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(4), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame = events(&next).remove(0);
    assert_eq!(frame["type"], "response.failed");
    assert_eq!(frame["sequence_number"], 1);
    assert_eq!(
        frame["response"]["error"]["code"],
        "agent_command_authority_revoked"
    );
    assert!(body.next().await.is_none());
    assert_eq!(
        f.agent,
        f.store.get_agent(&f.agent.id).await.unwrap().unwrap()
    );
    drop(replicas);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_response_historical_model_usage_and_post_terminal_tools_match_complete_http_replay()
{
    let (mut f, id) = retained(1_000).await;
    for (seq, kind) in [
        (
            994,
            AgentLogKind::ToolCallStarted {
                call_id: "retained-tool".into(),
                title: "tool".into(),
                kind: "execute".into(),
                status: "running".into(),
            },
        ),
        (995, AgentLogKind::TurnCompleted { turn_sequence: 991 }),
        (
            996,
            AgentLogKind::ToolCallUpdated {
                call_id: "retained-tool".into(),
                title: None,
                status: "completed".into(),
                output: None,
            },
        ),
        (
            997,
            AgentLogKind::ToolOutputReported {
                call_id: "retained-tool".into(),
                output: RecordedToolOutput {
                    text: "after terminal".into(),
                    truncated: false,
                },
            },
        ),
        (
            998,
            AgentLogKind::Usage {
                input_tokens: 7,
                output_tokens: 11,
                context_used: 18,
                context_window: None,
            },
        ),
        (
            999,
            AgentLogKind::TurnStarted {
                turn_sequence: 999,
                origin_key: None,
                model: Some("later-model".into()),
            },
        ),
        (
            1000,
            AgentLogKind::OwnerMessage {
                content: "later request".into(),
            },
        ),
    ] {
        f.store
            .agent_log
            .replace_one(
                doc! {"agent_id":f.agent.id.as_str(),"seq":seq},
                log_document(&AgentLogEntry {
                    agent_id: f.agent.id.clone(),
                    seq: seq as u64,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    f.agent.conversation.active_turn = Some(999);
    f.agent.revision += 1;
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    f.store = f
        .store
        .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
        .await
        .unwrap();
    let client = client();
    let path = format!("/api/v2/responses/{id}");
    let closed = servers_mode(&f, false).await;
    let expected = request(&client, &f, &closed[0], &path)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let old = request(&client, &f, &closed[0], &format!("{path}/events"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(expected["model"], "retained-model");
    assert_eq!(expected["usage"]["total_tokens"], 18);
    assert_eq!(expected["status"], "completed");
    assert!(expected.to_string().contains("after terminal"));
    drop(closed);
    let shared = servers_mode(&f, true).await;
    for server in &shared {
        assert_eq!(
            request(&client, &f, server, &path)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap(),
            expected
        );
        let new = request(&client, &f, server, &format!("{path}/events"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(
            new, old,
            "wire bytes include exact replay ids and terminal evidence"
        );
    }
    let all = events(&old);
    for after in 0..all.len() {
        let suffix = request(
            &client,
            &f,
            &shared[after % 2],
            &format!("{path}/events?starting_after={after}"),
        )
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
        assert_eq!(events(&suffix), all[after + 1..]);
    }
    drop(shared);
    f.close().await;
}
