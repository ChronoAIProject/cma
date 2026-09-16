use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_grant_mongo_write_conflict_preserves_transaction_retry_classification() {
    let f = crate::store::bridge_invokes::outbox::tests::enabled_fixture().await;
    let grants = &f.store.grants;
    let row = grants.find_one(doc! {}).await.unwrap().unwrap();
    let filter = doc! {"_id":row.get_str("_id").unwrap()};
    let mut session = f.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    grants
        .find_one(filter.clone())
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    // Another transaction wins a write after this snapshot was established.
    // This yields the actual driver's TransientTransactionError label.
    grants
        .update_one(filter.clone(), doc! {"$inc":{"fixture_conflict":1_i64}})
        .await
        .unwrap();
    let driver = grants
        .update_one(filter, doc! {"$inc":{"outbox_fence":1_i64}})
        .session(&mut session)
        .await
        .unwrap_err();
    assert!(crate::store::sandbox_mongo::is_transient(&driver));
    let mapped = grant_error(crate::store::bridge_invokes::MutationError::Mongo(driver));
    assert!(mapped.retryable());
    assert!(matches!(mapped, MutationError::Driver(_)));
    let _ = session.abort_transaction().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn pre_send_deferral_preserves_committed_sending_and_its_rate_lease() {
    use crate::store::bridge_invokes::outbox::tests::{retained, retained_item, seed, store};
    let f = crate::store::bridge_invokes::outbox::tests::enabled_fixture().await;
    let outbox = store(&f).await;
    let item = retained_item(&f, "claim-became-sending");
    seed(&outbox, &item).await;
    let holder = "claimed-before-unknown-commit";
    let claimed = outbox.claim_item(&item.id, holder).await.unwrap().unwrap();
    let rate_before = outbox.send_rates.find_one(doc! {}).await.unwrap().unwrap();
    let mut sending = claimed.clone();
    let DeliveryState::Claimed { attempt, .. } = &claimed.state else {
        unreachable!()
    };
    sending.begin_send(holder, attempt, now_ms()).unwrap();
    // Model the durable outcome of a successful Sending commit whose response
    // was lost. The caller still holds its earlier exact Claimed snapshot.
    outbox
        .items
        .replace_one(
            doc! {"_id":&claimed.id,"revision":claimed.revision as i64},
            crate::store::bridge_invokes::outbox::records::item_document(&sending).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        outbox
            .defer_claim(&claimed, holder, DeliveryFailure::ConnectUnavailable)
            .await,
        Err(OutboxError::Conflict)
    );
    assert!(retained(&outbox, &sending.id).await == sending);
    assert_eq!(
        outbox.send_rates.find_one(doc! {}).await.unwrap().unwrap(),
        rate_before
    );
    f.close().await;
}
