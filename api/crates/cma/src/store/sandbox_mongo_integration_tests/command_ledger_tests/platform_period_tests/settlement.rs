use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_turn_reserved_before_activation_settles_without_touching_the_period_row() {
    let (client, name) = database("platform_activation").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure_metered(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "pre-activation"))
            .await
            .unwrap(),
    );
    // Reshape the reservation and the row as a predecessor image left them:
    // no period on the record, and a freshly activated row with no liability.
    let command = format!("command:{}", admitted.receipt.envelope.command_id.as_str());
    store
        .model_budget
        .update_one(
            doc! {"_id": &command},
            doc! {"$unset": {"record.period_start": ""}},
        )
        .await
        .unwrap();
    store
        .model_budget
        .delete_one(doc! {"_id": {"$regex": "^platform:"}})
        .await
        .unwrap();
    observe(&store, &open_platform(), 0, seconds(NOW)).await;
    let activated = period_row(&store, &open_platform()).await.unwrap();
    start_turn(&store, execution, &admitted).await;
    finish_turn(&store, &agent).await;
    let owner = store
        .model_budget
        .find_one(doc! {"_id": format!("owner:{}", agent.owner.as_str())})
        .await
        .unwrap()
        .unwrap();
    let owner: layer2_core::budget::OwnerBudget =
        crate::store::sandbox_mongo::read_record(&owner).unwrap();
    assert_eq!((owner.reserved, owner.spent), (0, policy.turn_limit));
    assert_eq!(
        period_row(&store, &open_platform()).await.unwrap(),
        activated
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lowering_the_cap_below_committed_refuses_the_next_admission_without_releasing() {
    let (client, name) = database("platform_lowered").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let turn_limit = configure_capped(&store, &agent, u64::from(u32::MAX)).await;
    assert!(store
        .reserve_command(request(&agent, &parent, "admitted"))
        .await
        .is_ok());
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.platform.period_cap": 10_i64},
            "$inc": {"revision": 1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "after-lowering"))
            .await,
        Err(CommandLedgerError::Budget(BudgetError::PlatformExhausted))
    );
    let row = period_row(&store, &capped(10)).await.unwrap();
    assert_eq!(row.reserved, turn_limit);
    let stop = row
        .closures
        .iter()
        .find(|closure| closure.reason == ClosureReason::SpendStop)
        .unwrap();
    assert_eq!(stop.settings_revision, 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn settlement_after_period_end_debits_the_original_row() {
    let (client, name) = database("platform_cross_period").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure_metered(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "spans-periods"))
            .await
            .unwrap(),
    );
    start_turn(&store, execution, &admitted).await;
    // The operator rotates to the next period before the turn settles.
    let next = PlatformBudgetPolicy {
        period_start: "2100-01-01T00:00:00Z".into(),
        period_end: "2101-01-01T00:00:00Z".into(),
        ..open_platform()
    };
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.platform":
            mongodb::bson::serialize_to_bson(&next).unwrap()},
              "$inc": {"revision": 1_i64}},
        )
        .await
        .unwrap();
    finish_turn(&store, &agent).await;
    let original = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((original.reserved, original.spent), (0, policy.turn_limit));
    assert!(period_row(&store, &next).await.is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_lost_runtime_mid_turn_keeps_period_reserved_until_terminal_evidence() {
    let (client, name) = database("platform_lost_runtime").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let policy = configure_metered(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "lost-runtime"))
            .await
            .unwrap(),
    );
    start_turn(&store, execution, &admitted).await;
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let generation = working.conversation.runtime_generation.unwrap();
    // The runtime disappears: no terminal evidence arrives and its usage ages
    // out. The stop path may ask for a cancel; nothing releases liability.
    let later = seconds(&layer2_core::now_rfc3339()) + 3_600;
    use layer2_core::budget::SpendSource;
    let stop = store
        .stop_requested(&agent.id, generation, later)
        .await
        .unwrap();
    assert!(stop.is_some());
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (policy.turn_limit, 0));
    finish_turn(&store, &agent).await;
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!((row.reserved, row.spent), (0, policy.turn_limit));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn disabled_or_absent_platform_policy_changes_nothing() {
    let (client, name) = database("platform_disabled").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure_metered(&store, &agent.owner).await;
    store
        .model_budget
        .delete_many(doc! {"_id": {"$regex": "^platform:"}})
        .await
        .unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"settings.model_budget.enabled": false}},
        )
        .await
        .unwrap();
    assert!(store
        .reserve_command(request(&agent, &parent, "disabled"))
        .await
        .is_ok());
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$unset": {"settings.model_budget": ""}},
        )
        .await
        .unwrap();
    assert!(store
        .reserve_command(request(&agent, &parent, "absent"))
        .await
        .is_ok());
    assert_eq!(
        store.model_budget.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owners_priced_differently_share_one_period_total() {
    let (client, name) = database("platform_valuations").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    configure_metered(&store, &agent.owner).await;
    let mut other_parent = active_sandbox("sbx_platform_priced");
    other_parent.owner = layer2_core::SubjectId("platform-priced-owner".into());
    store.create_sandbox(other_parent.clone()).await.unwrap();
    let mut other = ready_agent(&other_parent, "agt_platform_priced");
    other.capabilities = budget_capabilities();
    insert_agent(&store, &other, NOW).await;
    // The second owner prices tokens three times higher; both allowances are
    // already valued amounts in the shared accounting units.
    let entry = layer2_core::budget::OwnerBudgetPolicy {
        owner: other.owner.clone(),
        policy: layer2_core::budget::BudgetPolicy {
            owner_day_limit: 1_000,
            turn_limit: 600,
            max_calls_per_turn: 255,
            day_timezone: "UTC".into(),
            input_token_units: 3,
            output_token_units: 3,
            max_unobserved_units: 1,
            usage_timeout_seconds: 60,
        },
    };
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$push": {"settings.model_budget.owners":
            mongodb::bson::serialize_to_bson(&entry).unwrap()}},
        )
        .await
        .unwrap();
    assert!(store
        .reserve_command(request(&agent, &parent, "unit-priced"))
        .await
        .is_ok());
    assert!(store
        .reserve_command(request(&other, &other_parent, "triple-priced"))
        .await
        .is_ok());
    let row = period_row(&store, &open_platform()).await.unwrap();
    assert_eq!(row.reserved, u64::from(u8::MAX) + 600);
    assert!(row.closures.is_empty());
    client.database(&name).drop().await.unwrap();
}
