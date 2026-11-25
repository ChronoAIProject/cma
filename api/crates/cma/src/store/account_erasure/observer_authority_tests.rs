use super::*;
const OBSERVERS: &str = crate::store::agent_observation::COLLECTION;

pub(super) async fn verify() {
    disabled_absence_issues_no_observer_commands().await;
    denied_authority_and_unknown_mode_refuse().await;
    resumed_namespace_is_rechecked_and_erased().await;
}

async fn once(fixture: &Fixture, mode: Option<bool>, cursor: &mut Cursor) -> Result<Batch, Error> {
    cursor.test_drain_seconds.get_or_insert(0);
    super::super::erase_records_with_options(
        &fixture.runtime,
        &[Target {
            subject: &SubjectId::new("acct:a"),
            inventory: &ErasureInventory {
                agents: vec!["agt_a".into()],
                ..Default::default()
            },
        }],
        cursor,
        NOW,
        Options {
            transaction_drain_minimum_secs: 120,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: mode,
            retain_legal_consents: false,
        },
    )
    .await
}

fn refusal(error: Error, expected: &str) {
    assert!(
        matches!(error, Error::CohortBlocked(CohortBlock::CollectionAuthority {
        collection: OBSERVERS, operation,
    }) if operation == expected),
        "expected observer {expected} refusal: {error:?}"
    );
}

async fn disabled_absence_issues_no_observer_commands() {
    let fixture = Fixture::for_collection(OBSERVERS, false, None, None).await;
    put(&fixture.admin, "users", doc! {"_id":"acct:a"}).await;
    let deleted = drain(&fixture.runtime, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(deleted.get("users"), Some(&1));
    fixture.absent().await;
    {
        let commands = fixture.commands.lock().unwrap();
        for command in commands.iter() {
            for operation in [
                "createIndexes",
                "create",
                "find",
                "delete",
                "aggregate",
                "listIndexes",
            ] {
                assert_ne!(command.get_str(operation).ok(), Some(OBSERVERS));
            }
        }
    }
    fixture.admin.drop().await.unwrap();
}

async fn denied_authority_and_unknown_mode_refuse() {
    for (grants, omit, missing, present, mode, operation) in [
        (false, None, None, true, Some(false), "find"),
        (true, None, Some("remove"), true, Some(false), "remove"),
        (false, Some(""), None, false, Some(false), "listCollections"),
        (false, None, None, false, Some(true), "find"),
        (false, None, None, false, None, "find"),
    ] {
        let fixture = Fixture::for_collection(OBSERVERS, grants, omit, missing).await;
        if present {
            put(&fixture.admin, OBSERVERS, doc! {"_id":"agt_a"}).await;
        }
        // Allocation metadata needs the same database grant and is checked first.
        let result = if omit.is_some() {
            super::super::optional::absent(&fixture.runtime, OBSERVERS, mode)
                .await
                .map(|_| Batch::default())
        } else {
            once(&fixture, mode, &mut Cursor::default()).await
        };
        refusal(result.unwrap_err(), operation);
        assert_eq!(count(&fixture.admin, OBSERVERS).await, u64::from(present));
        fixture.admin.drop().await.unwrap();
    }
}

async fn resumed_namespace_is_rechecked_and_erased() {
    for grants in [false, true] {
        let fixture = Fixture::for_collection(OBSERVERS, grants, None, None).await;
        let mut cursor = Cursor::default();
        assert!(
            once(&fixture, Some(false), &mut cursor)
                .await
                .unwrap()
                .has_more
        );
        fixture.absent().await;
        let operations = super::super::progress::operations();
        cursor.operation = operations.len();
        super::super::progress::bind(&mut cursor, &operations);
        cursor = mongodb::bson::deserialize_from_document(
            mongodb::bson::serialize_to_document(&cursor).unwrap(),
        )
        .unwrap();
        put(&fixture.admin, OBSERVERS, doc! {"_id":"agt_a"}).await;
        put(&fixture.admin, OBSERVERS, doc! {"_id":"agt_b"}).await;
        if grants {
            let mut removed = 0;
            let mut finished = false;
            for _ in 0..2000 {
                let batch = once(&fixture, Some(false), &mut cursor).await.unwrap();
                removed += batch.deleted.get(OBSERVERS).copied().unwrap_or(0);
                if !batch.has_more {
                    finished = true;
                    break;
                }
            }
            assert!(finished);
            assert_eq!(removed, 1);
            assert!(fixture
                .admin
                .collection::<Document>(OBSERVERS)
                .find_one(doc! {"_id":"agt_b"})
                .await
                .unwrap()
                .is_some());
        } else {
            refusal(
                once(&fixture, Some(false), &mut cursor).await.unwrap_err(),
                "find",
            );
            assert_eq!(count(&fixture.admin, OBSERVERS).await, 2);
        }
        fixture.admin.drop().await.unwrap();
    }
}
