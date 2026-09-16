use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_reference_failure_rolls_back_command_and_permanent_acceptance_binding() {
    let c = ChannelFixture::new().await;
    let db = c.f.client.database(&c.f.database);
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let before = receipts.find_one(doc! {"_id": &c.inbox.id}).await.unwrap();
    let allocations = db.collection::<Document>("bridge_allocations");
    let allocation = allocations
        .find_one(doc! {"_id": &c.allocation.id})
        .await
        .unwrap()
        .unwrap();
    db.run_command(doc! {"collMod":"channel_history_references", "validator":{"denied":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    assert!(c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .is_err());
    assert_eq!(
        db.collection::<Document>("agent_commands")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        receipts.find_one(doc! {"_id": &c.inbox.id}).await.unwrap(),
        before
    );
    let after = allocations
        .find_one(doc! {"_id": &c.allocation.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.get("channel_history_reference_count"),
        allocation.get("channel_history_reference_count")
    );
    db.run_command(doc! {"collMod":"channel_history_references", "validator":{}})
        .await
        .unwrap();
    let command = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let count = db
        .collection::<Document>("channel_history_references")
        .count_documents(doc! {"allocation_id": &c.allocation.id})
        .await
        .unwrap();
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        command
    );
    assert_eq!(
        db.collection::<Document>("channel_history_references")
            .count_documents(doc! {"allocation_id": &c.allocation.id})
            .await
            .unwrap(),
        count
    );
    c.f.client.database(&c.f.database).drop().await.unwrap();
}
