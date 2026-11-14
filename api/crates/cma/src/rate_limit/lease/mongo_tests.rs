use super::*;
use crate::store::request_limits::{MongoRequestLimits, COLLECTION};
use mongodb::bson::{doc, Document};
use std::sync::atomic::{AtomicU32, Ordering};

mod fixtures;
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
    assert_eq!(updates.len(), 8); // first request creates an empty bucket and retries
    for command in updates {
        // Independent literals pin the old single-token Mongo predicate and update.
        assert_eq!(
            command.get_document("query").unwrap(),
            &doc! {
                "_id": &key, "$expr": {"$lte": ["$tat", {"$add": [{"$toLong": "$$NOW"}, 59_900_i64]}]}
            }
        );
        assert_eq!(
            command.get_array("update").unwrap(),
            &vec![mongodb::bson::Bson::Document(doc! {
                "$set": {"tat": {"$add": [{"$max": ["$tat", {"$toLong": "$$NOW"}]}, 100_i64]},
                "expires_at": {"$add": ["$$NOW", 120_000]}}
            })]
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
    prepaid: AtomicU32,
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
    let fixture = Fixture::new().await;
    let counted = Counted {
        store: fixture.store.clone(),
        prepaid: AtomicU32::new(0),
    };
    for round in 0..100 {
        let left = LeaseTable::default();
        let right = LeaseTable::default();
        let key = format!("default:boundary-{round}");
        history(&left, &key, 2).await;
        history(&right, &key, 2).await;
        counted.prepaid.store(0, Ordering::SeqCst);
        let started = Instant::now();
        let attempts = (0..44).map(|index| {
            let table = if index % 2 == 0 { &left } else { &right };
            table.check(&counted, &key, 40)
        });
        let results = futures_util::future::join_all(attempts).await;
        let admitted = results.iter().filter(|result| result.is_ok()).count() as u32;
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "fixture crossed a refill interval"
        );
        assert_eq!(admitted, 40, "round {round}");
        assert_eq!(counted.prepaid.load(Ordering::SeqCst), 40);
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
    let fixture = Fixture::new().await;
    let left = RateLimiters::from_config(
        config(true),
        crate::config::TrustedProxyConfig::sample(),
        fixture.store.clone(),
    );
    let right = RateLimiters::from_config(
        config(true),
        crate::config::TrustedProxyConfig::sample(),
        fixture.store.clone(),
    );
    for (budget, limit) in [
        (Budget::Create, 10),
        (Budget::Input, 120),
        (Budget::Control, 120),
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
            assert_eq!(commands.len(), subjects * if enabled { 5 } else { 23 });
        }
    }
    fixture.database.drop().await.unwrap();
}
