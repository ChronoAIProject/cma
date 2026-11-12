use super::*;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;

async fn apply_output(
    store: &MongoSandboxStore,
    current: &Agent,
    next: &AgentMessageCheckpoint,
    kind: Write,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    match kind {
        Write::Append => {
            crate::store::agent_log::append_in_transaction(
                store,
                session,
                &current.id,
                AT.into(),
                AgentLogKind::AgentMessage {
                    content: "retained output".into(),
                },
            )
            .await?;
        }
        Write::Conversation => {
            apply_mutation(
                ScanCaller::ConversationPublication,
                store,
                &update(current, "retained output"),
                session,
            )
            .await?;
        }
        Write::Checkpoint => {
            apply(store, current, None, next, None, &Authority::Live, session).await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn committed_hold_invalidates_every_output_snapshot_and_refuses_fresh_retry() {
    for kind in WRITES {
        for v1 in [false, true] {
            let (client, name, store, parent, current) = fixture().await;
            let next = partial(&current, "retained output");
            let mut session = client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            store
                .agents
                .find_one(doc! {"_id":current.id.as_str()})
                .session(&mut session)
                .await
                .unwrap()
                .unwrap();
            let other = MongoSandboxStore::open(&client, &name).await.unwrap();
            maintenance::hold(&other, &parent, v1).await;
            let held = support::all_rows(&store).await;
            assert!(
                matches!(apply_output(&store, &current, &next, kind, &mut session).await,
                Err(ApplyError::Driver(error)) if is_transient(&error))
            );
            let _ = session.abort_transaction().await;
            assert_eq!(
                write_output(&store, &current, &next, kind).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(support::all_rows(&store).await, held);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and output hooks"]
async fn every_output_commit_invalidates_the_old_admission_snapshot_and_source() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    for kind in WRITES {
        let (client, name, store, parent, current) = fixture().await;
        let (policy, input) = support::pre_exclusion(&store, &parent, support::NOW).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        crate::store::ledger_maintenance_snapshot::load_pre_exclusion(
            &store,
            &input.caller,
            &parent.id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        write_output(
            &other,
            &current,
            &partial(&current, "retained output"),
            kind,
        )
        .await
        .unwrap();
        let written = support::all_rows(&store).await;
        assert!(
            matches!(crate::store::ledger_maintenance_pre_exclusion_transaction::apply(
            &store, &policy, &Change::Admit(input.clone()), &mut session).await,
            Err(HoldError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.admit_pre_exclusion(policy, input).await,
            Err(LedgerMaintenanceStoreError::Refused(
                LedgerMaintenanceAdmissionRefusal::Conflict
            ))
        );
        assert_eq!(support::all_rows(&store).await, written);
        client.database(&name).drop().await.unwrap();
    }
}
