use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_bot_keeps_custody_identifiers_and_rejects_stale_redaction() {
    use layer2_core::bots::{BotOwnership, BotStore};
    let db = database().await;
    let store = crate::store::bots::MongoBotStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut bot = crate::store::bots_mongo_integration_tests::bot("organization", "a");
    bot.ownership = Some(BotOwnership {
        owner: policy().owner,
        created_by: ResourceActor::Api {
            subject: SubjectId::new("erased"),
            delegation_id: Some("organization-delegation".into()),
        },
        credential_subject: SubjectId::new("erased"),
        credential_incarnation: Some("organization-sign-in".into()),
        authorization_grant_id: Some("organization-delegation".into()),
    });
    bot.resources.credentials.agent_key_id = Some("organization-agent-key".into());
    bot.resources.credentials.route_key_id = Some("organization-route-key".into());
    bot.validate().unwrap();
    store.insert(&bot).await.unwrap();
    let stale = row(&db, "bots", &bot.bot_id).await;
    let mut concurrent = bot.clone();
    concurrent.revision = 2;
    concurrent.updated_at = NOW.into();
    store.replace(1, &concurrent).await.unwrap();
    assert_eq!(
        redact(&db, "bots", &stale, &subjects()).await.unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "bots", &bot.bot_id).await;
    assert_eq!(
        redact(&db, "bots", &current, &subjects()).await.unwrap(),
        Redaction::Updated
    );
    let cleaned = store.get(&bot.bot_id).await.unwrap().unwrap();
    cleaned.validate().unwrap();
    assert_eq!(cleaned.owner, SubjectId::new("organization"));
    assert_eq!(cleaned.resource_owner(), policy().owner);
    assert_eq!(
        cleaned.ownership.as_ref().unwrap().created_by,
        actor("deleted-account")
    );
    assert_eq!(
        cleaned.credential_subject(),
        &SubjectId::new("deleted-account")
    );
    assert_eq!(cleaned.credential_incarnation(), None);
    assert_eq!(cleaned.authorization_grant_id(), None);
    assert_eq!(cleaned.resources, bot.resources);
    assert_eq!(cleaned.updated_at, NOW);
    assert_eq!(cleaned.revision, 3);
    assert_eq!(cleaned.profiles, bot.profiles);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_bot_delegation_is_handed_off_before_personal_custody_is_erased() {
    use crate::store::account_deletions::MongoAccountDeletionStore;
    use layer2_core::account_deletion::{AccountDeletionStore, DelegationDebt};
    use layer2_core::bots::{BotOwnership, BotStore};
    use std::time::Duration;
    let db = database().await;
    let store = crate::store::bots::MongoBotStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut bot = crate::store::bots_mongo_integration_tests::bot("organization", "a");
    bot.ownership = Some(BotOwnership {
        owner: policy().owner,
        created_by: ResourceActor::Api {
            subject: SubjectId::new("erased"),
            delegation_id: Some("organization-delegation".into()),
        },
        credential_subject: SubjectId::new("erased"),
        credential_incarnation: Some("organization-sign-in".into()),
        authorization_grant_id: Some("organization-delegation".into()),
    });
    store.insert(&bot).await.unwrap();
    let original = row(&db, "bots", &bot.bot_id).await;
    let subject = SubjectId::new("erased");
    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let batch = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    let debt = DelegationDebt {
        bot_id: bot.bot_id.clone(),
        authorization_grant_id: "organization-delegation".into(),
        credential_incarnation: "organization-sign-in".into(),
    };
    assert_eq!(
        batch.pending_delegations.get("erased"),
        Some(&vec![debt.clone()])
    );
    assert!(batch.waiting);
    assert_eq!(row(&db, "bots", &bot.bot_id).await, original);
    // Restart from the serialized cursor before acknowledging the returned debt.
    cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
    let retry = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(
        retry.pending_delegations.get("erased"),
        Some(&vec![debt.clone()])
    );
    assert_eq!(row(&db, "bots", &bot.bot_id).await, original);
    inventory.pending_delegations.push(DelegationDebt {
        credential_incarnation: "different-sign-in".into(),
        ..debt.clone()
    });
    let incorrect = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(
        incorrect.pending_delegations.get("erased"),
        Some(&vec![debt.clone()])
    );
    assert_eq!(row(&db, "bots", &bot.bot_id).await, original);
    inventory.pending_delegations.clear();
    inventory.pending_delegations.push(debt.clone());
    let deletions = MongoAccountDeletionStore::open(&db).await.unwrap();
    let requested = deletions
        .request(&subject, "erase-bot-owner", NOW)
        .await
        .unwrap();
    let mut claimed = deletions
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, requested.id);
    let lease = claimed.work.lease.clone().unwrap();
    let revision = claimed.revision;
    claimed.inventory = inventory;
    claimed
        .renew(lease.fence, NOW, Duration::from_secs(90))
        .unwrap();
    deletions
        .commit(claimed, vec![], revision, Some(&lease.token))
        .await
        .unwrap();
    drop(deletions);
    let restarted = MongoAccountDeletionStore::open(&db).await.unwrap();
    let mut inventory = restarted
        .get(&requested.id)
        .await
        .unwrap()
        .unwrap()
        .inventory;
    assert_eq!(inventory.pending_delegations, std::slice::from_ref(&debt));
    drain(&db, "erased", &mut inventory).await;
    let cleaned = store.get(&bot.bot_id).await.unwrap().unwrap();
    assert_eq!(
        cleaned.credential_subject(),
        &SubjectId::new("deleted-account")
    );
    assert_eq!(inventory.pending_delegations, [debt]);
    db.drop().await.unwrap();
}
