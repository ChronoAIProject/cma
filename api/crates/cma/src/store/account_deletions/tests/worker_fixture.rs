pub(super) use super::*;
pub(super) use crate::store::account_deletions::worker::MongoErasureJournal;
pub(super) use layer2_core::account_deletion::worker::*;

pub(super) fn budget() -> JournalBudget {
    JournalBudget::new(Duration::from_secs(20)).unwrap()
}
pub(super) fn binding(id: AccountDeletionId) -> JournalBinding {
    JournalBinding {
        deletion_id: id,
        contract: JournalContract {
            version: 1,
            worker: "a".repeat(64).try_into().unwrap(),
            catalogue: "b".repeat(64).try_into().unwrap(),
            policy: "c".repeat(64).try_into().unwrap(),
        },
        sequence: 0,
        inventory_generation: 0,
        step: ErasureStep::Inventory,
    }
}
pub(super) fn plan(claim: &JournalClaim, suffix: &str) -> JournalRequest {
    JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Prepare {
            page: PlanPage {
                targets: vec![TargetReference {
                    class: TargetClass::Delegation,
                    namespace: "synthetic-delegations".into(),
                    identity: format!("target-{suffix}"),
                    incarnation: format!("incarnation-{suffix}"),
                }],
                cursor: Some(format!("page-{suffix}")),
            },
        },
    )
    .unwrap()
}
pub(super) fn recorded(outcome: JournalOutcome) -> (JournalReceipt, Option<JournalClaim>) {
    let JournalOutcome::Recorded(recorded) = outcome else {
        panic!("journal was not recorded")
    };
    (recorded.receipt, recorded.claim)
}
pub(super) async fn enrolled() -> (
    Database,
    MongoAccountDeletionStore,
    MongoErasureJournal,
    JournalCheckpoint,
) {
    let (_, db, store) = fixture().await;
    crate::store::migrations::account_erasure_work::prepare_for_test(&db).await;
    let at = database_now(&db).await;
    let original = store
        .request(&SubjectId::new("owner"), "accepted", &at)
        .await
        .unwrap();
    store
        .request(&SubjectId::new("foreign-owner"), "foreign", &at)
        .await
        .unwrap();
    let journal = MongoErasureJournal::from_database(&db);
    let request = JournalRequest::new(binding(original.id), JournalCommand::Enroll).unwrap();
    let (receipt, claim) = recorded(journal.enroll(&request, &budget()).await.unwrap());
    assert!(claim.is_none());
    (db, store, journal, receipt.checkpoint)
}
pub(super) async fn acquire(
    journal: &MongoErasureJournal,
    checkpoint: JournalCheckpoint,
    lease_seconds: u64,
) -> JournalClaim {
    let request = JournalRequest::new(
        checkpoint.binding,
        JournalCommand::Claim {
            request_id: uuid::Uuid::new_v4().to_string(),
            holder: "worker".into(),
            lease_seconds,
        },
    )
    .unwrap();
    recorded(journal.claim_due(&request, &budget()).await.unwrap())
        .1
        .unwrap()
}
pub(super) async fn claimed() -> (
    Database,
    MongoAccountDeletionStore,
    MongoErasureJournal,
    JournalClaim,
) {
    let (db, store, journal, checkpoint) = enrolled().await;
    let claim = acquire(&journal, checkpoint, 90).await;
    (db, store, journal, claim)
}
pub(super) async fn snapshot(db: &Database) -> Vec<Vec<Document>> {
    let mut out = vec![];
    for name in [RECORDS, EVENTS, worker_schema::WORK, worker_schema::EFFECTS] {
        out.push(
            db.collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    out
}
pub(super) async fn reopened(db: &Database, name: Option<&str>) -> MongoErasureJournal {
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = name.map(str::to_owned);
    options.server_selection_timeout = Some(Duration::from_secs(5));
    options.connect_timeout = Some(Duration::from_secs(5));
    let client = mongodb::Client::with_options(options).unwrap();
    MongoErasureJournal::from_database(&client.database(db.name()))
}
