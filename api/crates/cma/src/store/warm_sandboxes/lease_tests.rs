use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn legacy_cleanup_two_clients_refuse_expired_renewal_and_completion() {
    let (client, name, first, _, profile, clock) = fixture(1).await;
    let second_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let second = MongoSandboxStore::open(&second_client, &name)
        .await
        .unwrap();
    reconcile(&first, &profile, clock).await;
    first
        .retire_legacy_warmups(clock.stamp(0).as_str())
        .await
        .unwrap();
    let (at, until) = (clock.stamp(0), clock.stamp(60));
    let (a, b) = tokio::join!(
        first.claim_legacy_warm_cleanup(&at, &until),
        second.claim_legacy_warm_cleanup(&at, &until)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let original = a.or(b).unwrap();
    assert!(
        second
            .claim_legacy_warm_cleanup(clock.stamp(1000).as_str(), clock.stamp(1060).as_str())
            .await
            .unwrap()
            .is_none(),
        "caller clock cannot take a live server lease"
    );
    first
        .renew_warmup_work(&original, clock.stamp(60).as_str())
        .await
        .unwrap();
    first
        .renew_warmup_work(&original, clock.stamp(30).as_str())
        .await
        .unwrap();
    let row = first
        .warm_sandboxes
        .find_one(doc! {"_id":original.record.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("lease_until").unwrap(), clock.stamp(60));
    for invalid in ["invalid", "1970-01-01T00:00:00Z"] {
        assert!(first.renew_warmup_work(&original, invalid).await.is_err());
    }
    expire(&first, &original.record.id).await;
    assert!(first
        .renew_warmup_work(&original, clock.stamp(60).as_str())
        .await
        .is_err());
    let mut next = original.record.clone();
    next.state = WarmSandboxState::Retired;
    assert!(first
        .finish_warmup_work(
            &original,
            next.clone(),
            clock.stamp(0).as_str(),
            clock.stamp(0).as_str()
        )
        .await
        .is_err());
    let successor = second
        .claim_legacy_warm_cleanup(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(first
        .renew_warmup_work(&original, clock.stamp(60).as_str())
        .await
        .is_err());
    assert!(first
        .finish_warmup_work(
            &original,
            next.clone(),
            clock.stamp(0).as_str(),
            clock.stamp(0).as_str()
        )
        .await
        .is_err());
    second
        .finish_warmup_work(
            &successor,
            next,
            clock.stamp(0).as_str(),
            clock.stamp(0).as_str(),
        )
        .await
        .unwrap();
    assert!(second
        .renew_warmup_work(&successor, clock.stamp(60).as_str())
        .await
        .is_err());
    assert!(first
        .claim_legacy_warm_cleanup(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .is_none());
    assert_eq!(records(&first).await.len(), 1);
    client.database(&name).drop().await.unwrap();
}
