//! Real command admission does not change the aggregate revision. Its durable
//! generation must still wake inspection before either retry deadline.
use super::*;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::request;
use layer2_core::sandbox::agent_io::{
    AgentCommandStore, CommandLedgerError, CommandReservation, CommandReservationResult,
};

async fn admission(store: &MongoSandboxStore, id: &AgentId, seconds: u64) -> CommandReservation {
    let agent = store.get_agent(id).await.unwrap().unwrap();
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let mut reservation = request(&agent, &parent, "pacing-wake");
    reservation.admitted_at = stamp(seconds);
    reservation.queue_deadline = stamp(seconds + 600);
    assert!(matches!(
        store.reserve_command(reservation.clone()).await.unwrap(),
        CommandReservationResult::Created(_)
    ));
    // Prove this is normal admission, with no synthetic revision change.
    assert_eq!(store.get_agent(id).await.unwrap(), Some(agent));
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    reservation
}

async fn establish_hold(store: &MongoSandboxStore, agent: &Agent) {
    let observed = observation(store, agent).await;
    for seconds in [0, 30, 90, 210, 450] {
        fail(store, &agent.id, seconds, Some(observed.clone()), CLASS).await;
    }
    assert_eq!(until(store, &agent.id).await, stamp(750));
    assert!(store
        .due_agent_work(&stamp(451), 1)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .claim_agent_work(&agent.id, "worker", stamp(451), stamp(1051))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_new_command_wakes_maximum_hold_and_resets_after_restart() {
    let (client, name) = database("failure_pacing_admission").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "admission").await;
    establish_hold(&store, &agent).await;
    let reservation = admission(&store, &agent.id, 451).await;
    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.due_agent_work(&stamp(451), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    let changed = observation(&reopened, &agent).await;
    assert_eq!(changed.command_admission_generation, 1);
    fail(&reopened, &agent.id, 451, Some(changed), CLASS).await;
    assert_eq!(until(&reopened, &agent.id).await, stamp(481));
    // Idempotent replay is not new work and cannot continually defeat pacing.
    assert!(matches!(
        reopened.reserve_command(reservation).await.unwrap(),
        CommandReservationResult::Existing(_)
    ));
    let current = reopened.get_agent(&agent.id).await.unwrap().unwrap();
    let parent = reopened
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut refused = request(&current, &parent, "full-queue");
    refused.queue_capacity = 1;
    assert_eq!(
        reopened.reserve_command(refused).await,
        Err(CommandLedgerError::QueueFull)
    );
    assert_eq!(
        reopened
            .agent_command_admission_generation(&agent.id)
            .await
            .unwrap(),
        Some(1)
    );
    assert!(reopened
        .due_agent_work(&stamp(452), 1)
        .await
        .unwrap()
        .is_empty());
    assert!(reopened
        .claim_agent_work(&agent.id, "worker", stamp(452), stamp(1052))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        reopened
            .agent_commands
            .count_documents(doc! {"status": "queued"})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_admission_racing_publication_cannot_reinstall_old_hold() {
    let (client, name) = database("failure_pacing_admission_race").await;
    for before_publication in [true, false] {
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let agent = ready_conversation_fixture(&store, "admission_race").await;
        establish_hold(&store, &agent).await;
        let claimed = store
            .claim_agent_work(&agent.id, "worker", stamp(750), stamp(1350))
            .await
            .unwrap()
            .unwrap();
        let mut observed = observation(&store, &claimed).await;
        observed.agent_revision = claimed.revision;
        let mutation = retry_mutation(&claimed, 750, CLASS);
        if before_publication {
            // Command commits after inspection, before transactional publication.
            admission(&store, &agent.id, 750).await;
            store
                .commit_agent_work_retry(mutation.clone(), observed.clone())
                .await
                .unwrap();
        } else {
            // The other ordering: failure commits, then admission, then the
            // delayed optional hint writer. It must preserve observed generation 0.
            store.commit_agent_work(mutation.clone()).await.unwrap();
            admission(&store, &agent.id, 750).await;
            crate::store::agent_failure_pacing::record(&store, &mutation, &observed)
                .await
                .unwrap();
        }
        // A duplicate late hint write cannot acknowledge the unobserved command.
        crate::store::agent_failure_pacing::record(&store, &mutation, &observed)
            .await
            .unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            reopened.due_agent_work(&stamp(751), 1).await.unwrap(),
            vec![agent.id.clone()]
        );
        let current = observation(&reopened, &agent).await;
        fail(&reopened, &agent.id, 751, Some(current), CLASS).await;
        assert_eq!(until(&reopened, &agent.id).await, stamp(781));
        assert_eq!(
            reopened
                .agent_commands
                .count_documents(doc! {"status": "queued"})
                .await
                .unwrap(),
            1
        );
        client.database(&name).drop().await.unwrap();
    }
}
