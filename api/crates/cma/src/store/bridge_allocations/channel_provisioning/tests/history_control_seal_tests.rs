use super::*;
use crate::store::{
    channel_history::{self, HistoryHold},
    sandbox_mongo::start_transaction,
};

async fn inventory(c: &ControlFixture) -> Result<usize, HistoryHold> {
    let mut session = c.f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let db = c.f.client.database(&c.f.database);
    let row = db
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id": &c.f.allocation.id})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let allocation = crate::store::bridge_allocations::decode_record(&row).unwrap();
    let result = channel_history::assess_controls(&db, &allocation, &mut session)
        .await
        .unwrap()
        .map(|rows| rows.len());
    session.abort_transaction().await.unwrap();
    result
}
async fn terminal(c: &ControlFixture) -> AdmittedChannelControl {
    let held = worker::admit(c, false, "history-control").await;
    assert_eq!(
        inventory(c).await,
        Err(HistoryHold::ControlHistoryUnresolved)
    );
    denial::consent(c, 1, SourceControlAction::RevokeSenderControls).await;
    let (execution, effects) = worker::execution(c, None, false);
    worker::run(c, &held, execution).await;
    assert!(effects.lock().unwrap().is_empty());
    denial::assert_refused(c, &held, CommandFailureCode::AuthorityRevoked).await;
    denial::stored(c, &held).await
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_control_inventory_keeps_terminal_refusal_and_permanent_aliases() {
    let c = ControlFixture::new().await;
    assert_eq!(inventory(&c).await, Ok(1));
    let refused = terminal(&c).await;
    assert_eq!(inventory(&c).await, Ok(6));
    let db = c.f.client.database(&c.f.database);
    // The same allocation fence used by the final seal rejects new material,
    // while immutable callback aliases remain permissible retained extensions.
    db.collection::<Document>("bridge_allocations")
        .update_one(
            doc! {"_id": &c.f.allocation.id},
            doc! {"$set": {"channel_history_seal": "fixture-seal", "state": "retired", "record.state": "retired"}},
        )
        .await
        .unwrap();
    db.collection::<Document>("agent_commands")
        .update_one(
            doc! {"_id": refused.sidecar.command_id.as_str()},
            doc! {"$set": {"channel_history_seal": "fixture-seal"}},
        )
        .await
        .unwrap();
    let alias = c
        .write("history-retained-alias", 200, 12345, true, false)
        .await;
    let recovered = c.store.recover(&alias).await.unwrap().unwrap();
    assert!(recovered == refused);
    assert_eq!(inventory(&c).await, Ok(8));
    let again = c.store.recover(&alias).await.unwrap().unwrap();
    assert!(again == refused);
    assert_eq!(inventory(&c).await, Ok(8));
    let generic = c
        .write("history-generic-callback-alias", 200, 12345, true, false)
        .await;
    use layer2_core::bridge::ingress::CallbackInboxStore;
    let recovered = c.callbacks.accept(generic.into_inbox()).await.unwrap();
    assert_eq!(
        recovered,
        layer2_core::bridge::ingress::CallbackAcceptance::ControlAccepted(
            refused.sidecar.intent.expected_acceptance().unwrap()
        )
    );
    assert_eq!(inventory(&c).await, Ok(9));
    let (execution, effects) = worker::execution(&c, None, false);
    worker::run(&c, &refused, execution).await;
    assert!(effects.lock().unwrap().is_empty());
    assert!(denial::stored(&c, &refused).await == refused);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_control_missing_reference_alias_or_receipt_remains_held() {
    for collection in [
        "channel_history_references",
        "bridge_channel_control_aliases",
        "bridge_callback_dispositions",
    ] {
        let c = ControlFixture::new().await;
        terminal(&c).await;
        assert_eq!(inventory(&c).await, Ok(6));
        let filter = if collection == "channel_history_references" {
            doc! {"record.kind": "control_intent"}
        } else if collection == "bridge_callback_dispositions" {
            doc! {"record.version": 2_i32}
        } else {
            doc! {}
        };
        assert_eq!(
            c.f.client
                .database(&c.f.database)
                .collection::<Document>(collection)
                .delete_one(filter)
                .await
                .unwrap()
                .deleted_count,
            1
        );
        assert!(
            inventory(&c).await.is_err(),
            "missing {collection} cannot seal"
        );
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_control_ambiguous_delivery_remains_held() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "history-ambiguous-control").await;
    let (execution, effects) = worker::execution(&c, None, true);
    worker::run(&c, &held, execution).await;
    assert_eq!(effects.lock().unwrap().len(), 1);
    assert_eq!(
        inventory(&c).await,
        Err(HistoryHold::ControlHistoryUnresolved)
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_control_history_write_failure_aborts_entire_admission() {
    let c = ControlFixture::new().await;
    let db = c.f.client.database(&c.f.database);
    let before = db
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id": &c.f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    db.run_command(doc! {"collMod": "channel_history_references", "validator": {"record.kind": {"$ne": "control_intent"}}}).await.unwrap();
    assert!(c
        .store
        .reserve(c.request("history-aborted-control", 920, false).await)
        .await
        .is_err());
    for collection in [
        "bridge_channel_control_intents",
        "bridge_channel_control_aliases",
        "bridge_callback_dispositions",
    ] {
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(if collection == "bridge_callback_dispositions" {
                    doc! {"record.version": 2_i32}
                } else {
                    doc! {}
                })
                .await
                .unwrap(),
            0,
            "{collection}"
        );
    }
    assert_eq!(
        db.collection::<Document>("agent_commands")
            .count_documents(doc! {"channel_control_admission": {"$exists": true}})
            .await
            .unwrap(),
        0
    );
    let after = db
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id": &c.f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.get("channel_history_reference_count"),
        after.get("channel_history_reference_count")
    );
    db.run_command(doc! {"collMod": "channel_history_references", "validator": {}})
        .await
        .unwrap();
    c.store
        .reserve(c.request("history-aborted-control", 920, false).await)
        .await
        .unwrap();
    assert_eq!(
        inventory(&c).await,
        Err(HistoryHold::ControlHistoryUnresolved)
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_control_seal_fences_fresh_admission_atomically() {
    let c = ControlFixture::new().await;
    let request = c.request("history-after-seal", 921, false).await;
    let db = c.f.client.database(&c.f.database);
    db.collection::<Document>("bridge_allocations")
        .update_one(
            doc! {"_id": &c.f.allocation.id},
            doc! {"$set": {"channel_history_seal": "fixture-seal"}},
        )
        .await
        .unwrap();
    assert!(c.store.reserve(request).await.is_err());
    assert_eq!(
        db.collection::<Document>("bridge_channel_control_intents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        db.collection::<Document>("bridge_channel_control_aliases")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        db.collection::<Document>("agent_commands")
            .count_documents(doc! {"channel_control_admission": {"$exists": true}})
            .await
            .unwrap(),
        0
    );
    c.f.close().await;
}
