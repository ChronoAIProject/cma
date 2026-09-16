use super::*;
use layer2_core::bridge::status::*;
use layer2_core::resource_access::ResourceAccessService;
use mongodb::bson::Document;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_status_reads_exact_channel_command_after_callback_expiry_without_writing() {
    let f = ReplyFixture::new().await;
    let service = BridgePartitionService::new(
        f.svc.clone(),
        Arc::new(ResourceAccessService::new(
            f.resources.clone(),
            Arc::new(Authority),
        )),
        f.allocations.clone(),
    );
    let database = f.client.database(&f.database);
    let commands = database.collection::<Document>("agent_commands");
    let id = f.command.receipt.envelope.command_id.as_str();
    let original = commands.find_one(doc! {"_id":id}).await.unwrap().unwrap();
    let read = service
        .detail(
            &caller(),
            None,
            &f.allocation.identity.bridge_id,
            &f.allocation.id,
        )
        .await
        .unwrap();
    let PartitionActivityView::Available {
        pending,
        latest: Some(latest),
        source_unavailable,
        ..
    } = &read.activity
    else {
        panic!("available exact Channel command")
    };
    assert!(!source_unavailable);
    assert_eq!(pending.len(), 1);
    assert_eq!(latest.command_id, f.command.receipt.envelope.command_id);
    assert_eq!(
        latest.response_id,
        f.command.receipt.response_id.clone().unwrap()
    );
    assert_eq!(latest.status, AgentResponseStatus::InProgress);
    // The fixture allocation predates this accepted follow-up. A partition hash
    // cannot manufacture its first-source label from an unrelated later input.
    assert!(matches!(
        read.partition.source,
        PartitionSourceView::Unavailable
    ));
    assert_eq!(
        commands.find_one(doc! {"_id":id}).await.unwrap().unwrap(),
        original
    );
    database
        .collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id":&f.inbox.id})
        .await
        .unwrap();
    let archived = service
        .detail(
            &caller(),
            None,
            &f.allocation.identity.bridge_id,
            &f.allocation.id,
        )
        .await
        .unwrap();
    let PartitionActivityView::Available {
        pending,
        latest: Some(retained),
        source_unavailable,
        ..
    } = archived.activity
    else {
        panic!("archived source remains available")
    };
    assert!(!source_unavailable);
    assert_eq!(pending.len(), 1);
    assert_eq!(&retained, latest);
    assert_eq!(
        commands.find_one(doc! {"_id":id}).await.unwrap().unwrap(),
        original
    );
    // Legacy rows without source retention are explicitly incomplete; reads do
    // not recreate a callback or archive and do not expose unproved correlation.
    commands
        .update_one(doc! {"_id":id}, doc! {"$unset":{"channel_source":""}})
        .await
        .unwrap();
    let legacy = service
        .detail(
            &caller(),
            None,
            &f.allocation.identity.bridge_id,
            &f.allocation.id,
        )
        .await
        .unwrap();
    let PartitionActivityView::Available {
        pending,
        latest,
        source_unavailable,
        ..
    } = legacy.activity
    else {
        panic!("legacy activity remains typed")
    };
    assert!(source_unavailable);
    assert!(pending.is_empty() && latest.is_none());
    assert!(!commands
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
        .contains_key("channel_source"));
    use futures_util::TryStreamExt;
    let indexes: Vec<_> = commands
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    for name in [
        "channel_partition_pending",
        "channel_partition_latest",
        "channel_partition_source",
    ] {
        assert!(indexes.iter().any(|index| index
            .options
            .as_ref()
            .and_then(|options| options.name.as_deref())
            == Some(name)));
    }
    f.close().await;
}
