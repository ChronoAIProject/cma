use super::*;
use layer2_core::bridge::status::*;
use layer2_core::resource_access::ResourceAccessService;
use mongodb::bson::Document;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_usage_is_read_only_checks_orphan_heads_and_bounds_retained_inventory() {
    use futures_util::TryStreamExt;
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
    let collections = [
        "bridge_definitions",
        "bridge_allocations",
        "bridge_allocation_heads",
        "sandboxes",
        "workspaces",
        "worktrees",
        "agents",
        "agent_commands",
        "bridge_callback_inbox",
    ];
    let mut before = Vec::new();
    for name in collections {
        before.push(
            database
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
        );
    }
    let view = service
        .usage(&caller(), None, &f.allocation.identity.bridge_id)
        .await
        .unwrap();
    assert_eq!(view.allocations.as_ref().unwrap().retained, 1);
    assert_eq!(view.allocations.as_ref().unwrap().current, 1);
    assert_eq!(view.allocations.as_ref().unwrap().sandbox_reservations, 1);
    assert!(matches!(
        view.target_resources,
        BridgeResourceUsage::Available { .. }
    ));
    for (name, original) in collections.into_iter().zip(before) {
        let current = database
            .collection::<Document>(name)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(current, original, "usage changed {name}");
    }
    let allocations = database.collection::<Document>("bridge_allocations");
    let original_allocation = allocations
        .find_one(doc! {"_id":&f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    let mut retired = f.allocation.clone();
    retired.state = AllocationState::Retired;
    retired.revision += 1;
    let mut retired_row = original_allocation.clone();
    retired.validate().unwrap();
    retired_row.insert("revision", retired.revision as i64);
    retired_row.insert(
        "record",
        crate::store::sandbox_mongo::record_bson(&retired).unwrap(),
    );
    allocations
        .replace_one(doc! {"_id":&retired.id}, retired_row)
        .await
        .unwrap();
    let retained = service
        .usage(&caller(), None, &retired.identity.bridge_id)
        .await
        .unwrap();
    assert_eq!(
        retained.allocations.as_ref().unwrap().sandbox_reservations,
        1
    );
    assert_eq!(retained.allocations.as_ref().unwrap().retired, 1);
    assert!(retained
        .capacity_reasons
        .contains(&BridgeCapacityReason::RetiredSandboxReservationsRetained));
    allocations
        .replace_one(doc! {"_id":&retired.id}, original_allocation)
        .await
        .unwrap();
    // A retained head absent from allocation evidence must never lower the
    // observed partition count. The bounded query rejects the inconsistent set.
    let heads = database.collection::<Document>("bridge_allocation_heads");
    heads.insert_one(doc! {"_id":"orphan-usage-head", "bridge_id":f.allocation.identity.bridge_id.as_str(), "allocation_id":"orphan-usage-allocation"}).await.unwrap();
    assert!(matches!(
        service
            .usage(&caller(), None, &f.allocation.identity.bridge_id)
            .await,
        Err(PartitionStatusError::Corrupt)
    ));
    heads
        .delete_one(doc! {"_id":"orphan-usage-head"})
        .await
        .unwrap();
    // The limit is applied before allocation decoding or target walks. Beyond
    // it even unreadable retained rows must produce no purported complete total.
    let allocations = database.collection::<Document>("bridge_allocations");
    let original = allocations
        .find_one(doc! {"_id":&f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    let rows = (0..256).map(|index| {
        let mut row = original.clone();
        row.insert("_id", format!("usage-overflow-{index}"));
        row.insert("key_hash", format!("usage-overflow-{index}"));
        row
    });
    allocations.insert_many(rows).await.unwrap();
    let limited = service
        .usage(&caller(), None, &f.allocation.identity.bridge_id)
        .await
        .unwrap();
    assert!(limited.allocations.is_none());
    assert!(matches!(
        limited.target_resources,
        BridgeResourceUsage::InventoryLimited
    ));
    assert!(limited
        .capacity_reasons
        .contains(&BridgeCapacityReason::InventoryLimit));
    f.close().await;
}

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
