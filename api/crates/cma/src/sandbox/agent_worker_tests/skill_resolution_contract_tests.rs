use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Peer};
use layer2_core::sandbox::{agent_live_skills::effective, AgentSkillInstallStatus};
use layer2_core::{Authorization, Caller, Capability};

#[path = "skill_resolution_contract_tests/fixture.rs"]
mod fixture;
use fixture::{operations, ready, reference, reply};

async fn assert_conversation(harness: &Harness, original: &Agent, state: &Agent, events: u64) {
    let mut expected = original.conversation.clone();
    expected.last_seq += events;
    assert_eq!(state.conversation, expected);
    let appended = harness
        .store
        .agent_log(&original.id, original.conversation.last_seq, 100)
        .await
        .unwrap();
    assert_eq!(appended.len() as u64, events);
    assert!(appended
        .iter()
        .all(|entry| matches!(entry.kind, AgentLogKind::SkillInstallationChanged { .. })));
}

async fn assert_preserved(
    harness: &Harness,
    original: &Agent,
    requests: usize,
    events: u64,
) -> Agent {
    let state = harness.stored(&original.id).await;
    assert_eq!(state.status, AgentStatus::Ready);
    assert_eq!(state.profile, original.profile);
    assert_eq!(state.work.resolved_skills, original.work.resolved_skills);
    assert_eq!(state.work.receipts, original.work.receipts);
    assert_conversation(harness, original, &state, events).await;
    assert_eq!(
        effective(&state.work, &SkillId::new("research")),
        effective(&original.work, &SkillId::new("research"))
    );
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    let installed = &snapshot.homes[&AgentSlot::new(original.id.as_str())].skills;
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version.as_deref(), Some("1.0"));
    let observed = harness.backend.requests().await;
    assert!(
        observed[requests..]
            .iter()
            .all(|(_, request)| request.operation() == WireOperation::RuntimeStatus),
        "resolution failure may inspect runtime status but must not dispatch a mutation: {:?}",
        &observed[requests..]
    );
    state
}

#[tokio::test]
async fn invalid_http_material_preserves_installed_skill_until_explicit_retry_succeeds() {
    for fault in [
        "missing",
        "json",
        "schema",
        "oversized",
        "identity",
        "version",
    ] {
        let mut peer = Peer::start().await;
        let (harness, original, _) = ready(&mut peer).await;
        let (operations, caller) = operations(&harness);
        operations
            .install_skill(&caller, &original.id, reference("research", "1.1"))
            .await
            .unwrap();
        let requests = harness.backend.requests().await.len();
        let ((), ()) = tokio::join!(harness.run(&original.id, SystemTime::now()), async {
            let exchange = peer.next().await;
            match fault {
                "missing" => exchange.respond(404, "private upstream diagnostic"),
                "json" => exchange.respond(200, "private upstream diagnostic"),
                "schema" => exchange.json(serde_json::json!({"data":{"guid":"research"}})),
                "oversized" => exchange.respond(200, vec![b'x'; 1024 * 1024 + 1]),
                "identity" => exchange.json(serde_json::json!({"data":{
                    "guid":"other", "name":"research", "version":"1.1", "skillHash":"a".repeat(64)
                }})),
                "version" => exchange.json(serde_json::json!({"data":{
                        "guid":"research", "name":"research", "version":"1.0", "skillHash":"a".repeat(64)
                    }})),
                _ => unreachable!(),
            }
        },);
        let failed = assert_preserved(&harness, &original, requests, 2).await;
        let entry = failed.work.live_skills.last().unwrap();
        assert_eq!(entry.status, AgentSkillInstallStatus::Failed, "{fault}");
        assert!(entry.material.is_none());
        assert_eq!(
            entry.error.as_deref(),
            Some("skill_not_accessible_or_version_invalid")
        );
        assert_eq!(failed.work.step, AgentWorkStep::None);
        assert_eq!(peer.count(), 2, "one initial lookup and one failed upgrade");
        let failed_entry = entry.clone();
        operations
            .install_skill(&caller, &original.id, reference("research", "1.1"))
            .await
            .unwrap();
        let ((), ()) = tokio::join!(harness.run(&original.id, SystemTime::now()), async {
            reply(peer.next().await, "1.1")
        },);
        let installed = harness.stored(&original.id).await;
        assert_eq!(installed.work.live_skills[0], failed_entry);
        assert_eq!(
            installed.work.live_skills.last().unwrap().status,
            AgentSkillInstallStatus::Installed
        );
        assert_eq!(
            effective(&installed.work, &SkillId::new("research"))
                .unwrap()
                .version,
            "1.1"
        );
        assert_conversation(&harness, &original, &installed, 4).await;
        assert_eq!(peer.count(), 3);
        let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
        let skills = &snapshot.homes[&AgentSlot::new(original.id.as_str())].skills;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].version.as_deref(), Some("1.1"));
    }
}

#[tokio::test]
async fn request_timeout_keeps_original_installation_and_retries_the_same_intent() {
    for fault in ["header", "body", "custody"] {
        let mut peer = Peer::start().await;
        let (harness, original, custody) = ready(&mut peer).await;
        let (operations, caller) = operations(&harness);
        let pending = operations
            .install_skill(&caller, &original.id, reference("research", "1.1"))
            .await
            .unwrap();
        let requests = harness.backend.requests().await.len();
        let worker = harness.worker.clone();
        let id = original.id.clone();
        let now = SystemTime::now();
        let waiting = (fault == "custody").then(|| custody.stall());
        let task = tokio::spawn(async move { worker.run_one(&id, now).await.unwrap() });
        let held = if let Some(waiting) = waiting {
            tokio::time::timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(custody.calls.load(Ordering::SeqCst), 2);
            None
        } else {
            let exchange = peer.next().await;
            if fault == "body" {
                exchange.stall_body();
                None
            } else {
                Some(exchange)
            }
        };
        tokio::time::pause();
        let seconds = if fault == "custody" { 30 } else { 15 };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        tokio::time::advance(Duration::from_secs(seconds)).await;
        assert!(finish(task).await);
        assert_deadline(deadline);
        let retrying = assert_preserved(&harness, &original, requests, 1).await;
        let entry = retrying.work.live_skills.last().unwrap();
        assert_eq!(entry.status, AgentSkillInstallStatus::Pending);
        assert!(entry.material.is_none());
        assert_eq!(entry.preparation_retries, 1);
        assert_eq!(entry.reference, pending.work.live_skills[0].reference);
        assert_eq!(peer.count(), if fault == "custody" { 1 } else { 2 });
        assert_eq!(retrying.work.live_skills.len(), 1);
        drop(held);
        tokio::time::resume();
        let ((), ()) = tokio::join!(
            harness.run(&original.id, now + Duration::from_secs(seconds + 2)),
            async { reply(peer.next().await, "1.1") },
        );
        let installed = harness.stored(&original.id).await;
        let entry = installed.work.live_skills.last().unwrap();
        assert_eq!(entry.reference, pending.work.live_skills[0].reference);
        assert_eq!(entry.status, AgentSkillInstallStatus::Installed);
        assert_conversation(&harness, &original, &installed, 2).await;
        assert_eq!(peer.count(), if fault == "custody" { 2 } else { 3 });
        assert_eq!(installed.work.live_skills.len(), 1);
        assert_eq!(custody.calls.load(Ordering::SeqCst), 3);
    }
}

#[tokio::test]
async fn partial_initial_resolution_creates_no_home_and_keeps_existing_agent_untouched() {
    let mut peer = Peer::start().await;
    let (harness, original, _) = ready(&mut peer).await;
    let references = (0..4)
        .map(|index| reference(&format!("skill_{index}"), "2.0"))
        .collect();
    let admitted = harness.admit("partial", references).await;
    harness.run(&admitted.id, harness.now).await;
    let requests = harness.backend.requests().await.len();
    let ((), ()) = tokio::join!(harness.run(&admitted.id, harness.now), async {
        let mut exchanges = Vec::new();
        for _ in 0..4 {
            exchanges.push(peer.next().await);
        }
        for exchange in exchanges {
            if exchange.uri.path().ends_with("skill_3") {
                exchange.respond(404, "synthetic missing skill");
            } else {
                reply(exchange, "2.0");
            }
        }
    },);
    let failed = harness.stored(&admitted.id).await;
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "agent_skill_not_found"
    );
    assert!(failed.work.resolved_skills.is_none());
    assert!(!harness
        .backend
        .snapshot(&harness.runtime)
        .await
        .unwrap()
        .homes
        .contains_key(&AgentSlot::new(admitted.id.as_str())));
    assert_preserved(&harness, &original, requests, 0).await;
    assert_eq!(peer.count(), 5);
}
