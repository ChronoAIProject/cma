//! The already-admitted command's later mutations must share the hold fence.
use super::*;

#[derive(Clone, Copy)]
enum Mutation {
    Renew,
    Release,
    Reject,
}
const MUTATIONS: [Mutation; 3] = [Mutation::Renew, Mutation::Release, Mutation::Reject];

fn after(seconds: u64) -> String {
    layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(
                layer2_core::seconds_from_rfc3339(NOW).unwrap() + seconds,
            ),
    )
}

async fn change(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    mutation: Mutation,
) -> Result<(), CommandLedgerError> {
    let claim = &command.work.lease.as_ref().unwrap().claim;
    match mutation {
        Mutation::Renew => store.renew_command_claim(claim, after(10), after(40)).await,
        Mutation::Release => store.release_command_claim(claim, after(10)).await,
        Mutation::Reject => store
            .reject_command(claim, CommandFailureCode::AuthorityRevoked, after(10))
            .await
            .map(|_| ()),
    }
}

async fn claimed(
    store: &MongoSandboxStore,
) -> (Policy, LedgerPreExclusionAdmission, AdmittedAgentCommand) {
    let (policy, mut input) = fixture(store).await;
    let source = &input.accepted.plan().source;
    let agent = source.agents[0].clone();
    store
        .reserve_command(reservation(&agent, &source.sandbox, "claimed-input"))
        .await
        .unwrap();
    let command = store
        .claim_next_command(claim_request(&agent, NOW))
        .await
        .unwrap()
        .unwrap();
    refresh(store, &mut input).await;
    (policy, input, command)
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn unheld_renew_release_and_reject_commit_the_expected_original_command() {
    for mutation in MUTATIONS {
        let (client, name, store) = database().await;
        let (_, _, original) = claimed(&store).await;
        let parent_before = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
        change(&store, &original, mutation).await.unwrap();
        let result = store
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.receipt, original.receipt);
        assert_eq!(result.revision, original.revision + 1);
        match mutation {
            Mutation::Renew => {
                assert_eq!(result.status, original.status);
                assert_eq!(result.work.lease.as_ref().unwrap().expires_at, after(40));
            }
            Mutation::Release => {
                assert_eq!(result.status, original.status);
                assert!(result.work.lease.is_none());
            }
            Mutation::Reject => {
                assert_eq!(result.status, AgentResponseStatus::Failed);
                assert!(result.work.lease.is_none());
            }
        }
        let parent_after = store.sandboxes.find_one(doc! {}).await.unwrap().unwrap();
        assert_eq!(
            parent_after
                .get_i64("ledger_writer_admission_fence_v1")
                .unwrap(),
            parent_before
                .get_i64("ledger_writer_admission_fence_v1")
                .unwrap()
                + 1
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn live_claim_cannot_renew_release_or_reject_after_pre_exclusion_restart() {
    let (client, name, store) = database().await;
    let (policy, input, command) = claimed(&store).await;
    store.admit_pre_exclusion(policy, input).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = all_rows(&store).await;
    for mutation in MUTATIONS {
        assert_eq!(
            change(&reopened, &command, mutation).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(all_rows(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn v1_hold_refuses_later_claim_mutations_without_an_envelope_version_conflict() {
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionPolicy;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStore;
    let (client, name, store) = database().await;
    let mut input = support::fixture(&store, true).await;
    let source = &input.accepted.plan().source;
    let agent = source.agents[0].clone();
    store
        .reserve_command(reservation(&agent, &source.sandbox, "v1-claimed-input"))
        .await
        .unwrap();
    let command = store
        .claim_next_command(claim_request(&agent, NOW))
        .await
        .unwrap()
        .unwrap();
    let mut plan = input.accepted.plan().clone();
    plan.source = store
        .inspect_ledger_maintenance(&input.caller, &agent.sandbox_id)
        .await
        .unwrap()
        .source;
    input.accepted = support::artifact(&plan);
    store
        .admit_ledger_maintenance(LedgerMaintenanceAdmissionPolicy { enabled: true }, input)
        .await
        .unwrap();
    let before = all_rows(&store).await;
    for mutation in MUTATIONS {
        assert_eq!(
            change(&store, &command, mutation).await,
            Err(CommandLedgerError::NotAdmissible)
        );
        assert_eq!(all_rows(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn missing_agent_or_maintenance_audit_refuses_claim_mutations_without_writes() {
    for missing_agent in [false, true] {
        let (client, name, store) = database().await;
        let (policy, input, command) = claimed(&store).await;
        let expected = if missing_agent {
            store.agents.delete_many(doc! {}).await.unwrap();
            CommandLedgerError::NotFound
        } else {
            store.admit_pre_exclusion(policy, input).await.unwrap();
            transaction::audit_collection(&store)
                .delete_many(doc! {})
                .await
                .unwrap();
            CommandLedgerError::Corrupt
        };
        let before = all_rows(&store).await;
        for mutation in MUTATIONS {
            assert_eq!(
                change(&store, &command, mutation).await,
                Err(expected.clone())
            );
            assert_eq!(all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn held_reconciliation_of_an_undispatched_or_terminal_command_is_exact_and_read_only() {
    for terminal in [false, true] {
        let (client, name, store) = database().await;
        let (policy, mut input, original) = claimed(&store).await;
        let id = &original.receipt.envelope.command_id;
        if terminal {
            change(&store, &original, Mutation::Reject).await.unwrap();
            refresh(&store, &mut input).await;
        }
        let original = store.get_command(id).await.unwrap().unwrap();
        store.admit_pre_exclusion(policy, input).await.unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let before = all_rows(&store).await;
        assert_eq!(reopened.reconcile_command(id).await.unwrap(), original);
        assert_eq!(all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn command_update_failure_rolls_back_its_parent_fence_and_every_sidecar() {
    let (client, name, store) = database().await;
    let (_, _, command) = claimed(&store).await;
    client.database(&name).run_command(doc! {"collMod":store.agent_commands.name(), "validator":{"reject_fixture":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    let before = all_rows(&store).await;
    assert_eq!(
        change(&store, &command, Mutation::Renew).await,
        Err(CommandLedgerError::Unavailable)
    );
    assert_eq!(all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agent_commands.name(), "validator":{}})
        .await
        .unwrap();
    change(&store, &command, Mutation::Renew).await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn hold_commit_conflicts_with_an_open_command_writer_snapshot() {
    let (client, name, store) = database().await;
    let (policy, input, command) = claimed(&store).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &input.accepted.plan().sandbox_id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    other.admit_pre_exclusion(policy, input).await.unwrap();
    let before = all_rows(&store).await;
    // Exercise the same write guard in an already-open transaction, then the
    // real renewal port from a fresh transaction. Neither may persist a write.
    assert!(
        matches!(crate::store::ledger_maintenance_writer_ports::command_agent(
        &store, &command.receipt.envelope.agent_id, &mut session).await,
        Err(crate::store::agent_commands::ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(all_rows(&store).await, before);
    assert_eq!(
        change(&store, &command, Mutation::Renew).await,
        Err(CommandLedgerError::NotAdmissible)
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and command write hook"]
async fn renewal_commit_conflicts_with_an_open_maintenance_snapshot_and_stale_proposal() {
    let (client, name, store) = database().await;
    let (policy, input, command) = claimed(&store).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &input.accepted.plan().sandbox_id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    change(&other, &command, Mutation::Renew).await.unwrap();
    let before = all_rows(&store).await;
    assert!(
        matches!(transaction::apply(&store, &policy, &Change::Admit(input.clone()), &mut session).await,
        Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(all_rows(&store).await, before);
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(Domain::Refused(Refusal::Conflict))
    );
    assert_eq!(all_rows(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
