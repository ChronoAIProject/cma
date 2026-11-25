use super::*;
use crate::store::request_limits::{MongoRequestLimits, COLLECTION};
use mongodb::bson::{doc, Document};
use std::sync::atomic::{AtomicU32, Ordering};

mod fixtures;
mod public_relay;
mod read_lanes;
use fixtures::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn disabled_leases_preserve_exact_command_sequence_and_one_token_gcra() {
    use crate::rate_limit::{Budget, RateKey, RateLimiters};
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new().await;
    let key = format!("default:{:x}", Sha256::digest(b"subject:fixture-owner"));
    for _ in 0..7 {
        fixture.store.check(&key, 600).await.unwrap();
    }
    let original = normalized(fixture.take_commands());
    fixture
        .database
        .collection::<Document>(COLLECTION)
        .delete_many(doc! {})
        .await
        .unwrap();
    let limiter = RateLimiters::from_config(
        config(false),
        crate::config::TrustedProxyConfig::sample(),
        fixture.store.clone(),
    );
    for _ in 0..7 {
        limiter
            .check_operation(Budget::Default, &RateKey::Subject("fixture-owner".into()))
            .await
            .unwrap();
    }
    let disabled = normalized(fixture.take_commands());
    assert_eq!(disabled, original);
    assert!(limiter
        .default_leases
        .table
        .lock()
        .unwrap()
        .entries
        .is_empty());
    let updates: Vec<_> = disabled
        .iter()
        .filter(|row| row.contains_key("findAndModify"))
        .collect();
    assert_eq!(updates.len(), 7); // each request atomically creates or charges its bucket
    for command in updates {
        // Independent literals pin the conditional single-token GCRA update.
        assert_eq!(command.get_document("query").unwrap(), &doc! {"_id": &key});
        let eligible =
            doc! {"$lte":[{"$ifNull":["$tat",0_i64]}, {"$add":[{"$toLong":"$$NOW"},59_900_i64]}]};
        assert_eq!(
            command.get_array("update").unwrap(),
            &vec![mongodb::bson::Bson::Document(doc! {
                "$set": {
                    "document_schema_version":{"$ifNull":["$document_schema_version",1_i32]},
                    "tat":{"$cond":[eligible.clone(),{"$add":[{"$max":[{"$ifNull":["$tat",0_i64]},{"$toLong":"$$NOW"}]},100_i64]},"$tat"]},
                    "expires_at":{"$cond":[eligible,{"$max":[{"$add":["$$NOW",120_000]},{"$toDate":{"$add":[{"$max":[{"$ifNull":["$tat",0_i64]},{"$toLong":"$$NOW"}]},100_i64]}}]},"$expires_at"]}
                }
            })]
        );
        assert!(command.get_bool("upsert").unwrap());
        assert!(!command.get_bool("new").unwrap());
        assert_eq!(
            command.get_document("fields").unwrap(),
            &doc! {
                "_id":0,"wait_ms":{"$max":[0_i64,{"$subtract":[{"$ifNull":["$tat",0_i64]},{"$add":[{"$toLong":"$$NOW"},59_900_i64]}]}]}
            }
        );
        assert_eq!(
            command
                .get_document("writeConcern")
                .unwrap()
                .get_str("w")
                .unwrap(),
            "majority"
        );
    }
    assert!(matches!(
        fixture.store.charge("invalid", 40, 0).await,
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        fixture.store.charge("invalid", 40, 41).await,
        Err(StoreError::Malformed(_))
    ));
    assert!(fixture.take_commands().is_empty());
    fixture.database.drop().await.unwrap();
}

struct Counted {
    store: Arc<MongoRequestLimits>,
    prepaid: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl Charge for Counted {
    async fn charge(&self, key: &str, limit: u32, tokens: u32) -> Result<Option<u64>, StoreError> {
        let result = self.store.charge(key, limit, tokens).await?;
        if result.is_none() {
            self.prepaid.fetch_add(tokens, Ordering::SeqCst);
        }
        Ok(result)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_replicas_never_admit_unpaid_requests_at_the_boundary_over_100_rounds() {
    let mut fixture = Fixture::new().await;
    let clock = mongodb::bson::DateTime::now().timestamp_millis();
    Arc::get_mut(&mut fixture.store)
        .unwrap()
        .set_test_clock_millis(clock);
    let prepaid = Arc::new(AtomicU32::new(0));
    let left_store = Counted {
        store: fixture.store.clone(),
        prepaid: prepaid.clone(),
    };
    let peer = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let mut peer_store = MongoRequestLimits::open(&peer.database(fixture.database.name()))
        .await
        .unwrap();
    peer_store.set_test_clock_millis(clock);
    let right_store = Counted {
        store: Arc::new(peer_store),
        prepaid: prepaid.clone(),
    };
    let buckets = fixture.database.collection::<Document>(COLLECTION);
    for round in 0..100 {
        let left = LeaseTable::default();
        let right = LeaseTable::default();
        let key = format!("default:boundary-{round}");
        history(&left, &key, 2).await;
        history(&right, &key, 2).await;
        prepaid.store(0, Ordering::SeqCst);
        let attempts = (0..44).map(|index| {
            let (table, store) = if index % 2 == 0 {
                (&left, &left_store)
            } else {
                (&right, &right_store)
            };
            table.check(store, &key, 40)
        });
        let results = futures_util::future::join_all(attempts).await;
        let admitted = results.iter().filter(|result| result.is_ok()).count() as u32;
        let stored = buckets
            .find_one(doc! { "_id": &key })
            .await
            .unwrap()
            .unwrap();
        // Forty 1.5-second charges must fit in one minute of the shared token clock.
        assert_eq!(
            stored.get_i64("tat").unwrap(),
            clock + 60_000,
            "round {round}"
        );
        assert_eq!(admitted, 40, "round {round}");
        assert_eq!(prepaid.load(Ordering::SeqCst), 40);
        assert!(results
            .iter()
            .all(|result| matches!(result, Ok(()) | Err(Refusal::Limited(1..=2)))));
    }
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn batch_refusal_falls_back_to_exact_single_request_and_retry_after() {
    let fixture = Fixture::new().await;
    let table = LeaseTable::default();
    history(&table, "default:near", 2).await;
    fixture.boundary("default:near", 58_500).await;
    assert_eq!(
        table
            .check(fixture.store.as_ref(), "default:near", 40)
            .await,
        Ok(())
    );
    let retry = fixture
        .store
        .check("default:near", 40)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        table
            .check(fixture.store.as_ref(), "default:near", 40)
            .await,
        Err(Refusal::Limited(retry))
    );
    assert_eq!(
        table.entry("default:near").unwrap().lock().await.remaining,
        0
    );
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn enabled_leases_leave_create_input_and_control_exact() {
    use crate::rate_limit::{Budget, RateKey, RateLimiters};
    use futures_util::TryStreamExt as _;
    let mut fixture = Fixture::new().await;
    // Freeze token time so slow majority writes cannot cross the 6 s refill
    // interval. An old clock also makes accidental use for TTL detectable.
    Arc::get_mut(&mut fixture.store)
        .unwrap()
        .set_test_clock_millis(mongodb::bson::DateTime::now().timestamp_millis() - 300_000);
    let mut limits = config(true);
    limits.input_per_minute = std::num::NonZeroU32::new(10).unwrap();
    limits.control_per_minute = std::num::NonZeroU32::new(10).unwrap();
    let left = RateLimiters::from_config(
        limits,
        crate::config::TrustedProxyConfig::sample(),
        fixture.store.clone(),
    );
    let right = RateLimiters::from_config(
        limits,
        crate::config::TrustedProxyConfig::sample(),
        fixture.store.clone(),
    );
    for (budget, limit) in [
        (Budget::Create, 10),
        (Budget::Input, 10),
        (Budget::Control, 10),
    ] {
        let key = RateKey::Subject(format!("fixture-{limit}-{budget:?}"));
        for index in 0..limit {
            let limiter = if index % 2 == 0 { &left } else { &right };
            limiter.check_operation(budget, &key).await.unwrap();
        }
        assert!(matches!(
            right.check_operation(budget, &key).await,
            Err(Refusal::Limited(_))
        ));
    }
    // Compare each bucket's TTL with Mongo's real clock, not the injected clock.
    let buckets: Vec<Document> = fixture
        .database
        .collection::<Document>(COLLECTION)
        .aggregate(vec![
            doc! { "$project": { "expires_at": 1, "server_now": "$$NOW" } },
        ])
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(
        buckets.len() == 3
            && buckets.iter().all(|bucket| {
                bucket.get_datetime("expires_at").unwrap()
                    > bucket.get_datetime("server_now").unwrap()
            }),
        "all three buckets must retain a TTL in Mongo's real future: {buckets:?}"
    );
    assert!(left.default_leases.table.lock().unwrap().entries.is_empty());
    assert!(right
        .default_leases
        .table
        .lock()
        .unwrap()
        .entries
        .is_empty());
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn mongo_failure_cannot_mint_credits_but_existing_prepayment_remains_spendable() {
    let fixture = Fixture::new().await;
    let table = LeaseTable::default();
    history(&table, "default:paid", 3).await;
    table
        .check(fixture.store.as_ref(), "default:paid", 600)
        .await
        .unwrap();
    // A rejecting validator makes writes fail in this disposable collection
    // without affecting the Docker daemon or another test database.
    fixture
        .database
        .run_command(doc! {"collMod": COLLECTION,
            "validator": {"$expr": {"$eq": [1, 0]}}, "validationLevel": "strict"
        })
        .await
        .unwrap();
    table
        .check(fixture.store.as_ref(), "default:paid", 600)
        .await
        .unwrap();
    table
        .check(fixture.store.as_ref(), "default:paid", 600)
        .await
        .unwrap();
    assert_eq!(
        table
            .check(fixture.store.as_ref(), "default:paid", 600)
            .await,
        Err(Refusal::Unavailable)
    );
    assert_eq!(
        table
            .check(fixture.store.as_ref(), "default:new", 600)
            .await,
        Err(Refusal::Unavailable)
    );
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn report_mongo_command_ratios_for_one_and_one_hundred_warm_subjects() {
    let fixture = Fixture::new().await;
    for subjects in [1, 100] {
        for enabled in [false, true] {
            let table = LeaseTable::default();
            fixture.take_commands();
            for subject in 0..subjects {
                let key = format!("default:ratio-{subjects}-{enabled}-{subject}");
                // The separately tested observation window has seen 10 arrivals.
                history(&table, &key, 10).await;
                for _ in 0..20 {
                    if enabled {
                        table
                            .check(fixture.store.as_ref(), &key, 600)
                            .await
                            .unwrap();
                    } else {
                        assert_eq!(fixture.store.check(&key, 600).await.unwrap(), None);
                    }
                }
            }
            let commands = fixture.take_commands();
            let requests = subjects * 20;
            eprintln!("subjects={subjects} leased={enabled} requests={requests} mongo_commands={} ratio={:.3}", commands.len(), commands.len() as f64 / requests as f64);
            assert_eq!(commands.len(), subjects * if enabled { 2 } else { 20 });
        }
    }
    fixture.database.drop().await.unwrap();
}
