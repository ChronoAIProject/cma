use super::*;

fn query(kind: OptionKind) -> OptionQuery {
    OptionQuery {
        kind,
        search: String::new(),
        owner: None,
        selected: None,
        cursor: None,
        limit: Some(50),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_options_use_retained_creation_and_precise_directory_join_times() {
    let store = fixture().await;
    let mut rows = [state(0, 1000), state(1, 86_400)];
    for row in &mut rows {
        row.name = "Research".into();
        row.owner = SubjectId("alice".into());
    }
    seed(&store, &rows).await;
    store.collection("users").insert_many([
        doc! {"_id":"alice","display_name":"Alice","first_seen_at":"2026-11-01T12:30:00.001Z"},
        doc! {"_id":"other-alice","display_name":"Alice","first_seen_at":"2026-11-01T12:30:00.002Z"},
    ]).await.unwrap();
    let operator = scope("operator", true);
    for (created_at, expected) in [
        (86_400_i64, ["1970-01-01", "1970-01-02"]),
        (1060, ["1970-01-01 00:16 UTC", "1970-01-01 00:17 UTC"]),
        (1001, ["1970-01-01 00:16:40 UTC", "1970-01-01 00:16:41 UTC"]),
    ] {
        store
            .collection(super::super::super::sandbox_usage::STATES)
            .update_one(
                doc! {"_id":rows[1].sandbox_id.as_str()},
                doc! {"$set":{"record.created_at":created_at}},
            )
            .await
            .unwrap();
        let page = store
            .options(
                &operator,
                query(OptionKind::Sandbox),
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);
        for (item, at) in page.items.iter().zip(expected) {
            assert_eq!(item.label, "Research");
            assert_eq!(item.disambiguator, Some(format!("Alice · Created {at}")));
        }
    }
    // Two owners whose plain names and join dates tie still have human context.
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .update_one(
            doc! {"_id":rows[1].sandbox_id.as_str()},
            doc! {"$set":{"owner":"other-alice"}},
        )
        .await
        .unwrap();
    let owners = store
        .options(
            &operator,
            query(OptionKind::Owner),
            OwnerContextAccess::DirectoryRead,
        )
        .await
        .unwrap();
    for (item, fraction) in owners.items.iter().zip(["001", "002"]) {
        assert_eq!(item.label, "Alice");
        assert_eq!(
            item.disambiguator,
            Some(format!("Joined 2026-11-01 12:30:00.{fraction} UTC"))
        );
    }
    // Distinct subjects with equal display names use the same Sandbox policy.
    let sandboxes = store
        .options(
            &operator,
            query(OptionKind::Sandbox),
            OwnerContextAccess::DirectoryRead,
        )
        .await
        .unwrap();
    assert!(sandboxes.items.iter().all(|item| !item
        .disambiguator
        .as_deref()
        .unwrap()
        .contains("reference")));
    let selected = store
        .options(
            &operator,
            OptionQuery {
                selected: Some(rows[0].sandbox_id.as_str().into()),
                ..query(OptionKind::Sandbox)
            },
            OwnerContextAccess::DirectoryRead,
        )
        .await
        .unwrap();
    assert_eq!(selected.items[0].label, "Research");
    assert!(selected.items[0].disambiguator.is_none());
    // Equal creation seconds have exhausted the precision retained by the ledger.
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .update_one(
            doc! {"_id":rows[1].sandbox_id.as_str()},
            doc! {"$set":{"record.created_at":1000_i64}},
        )
        .await
        .unwrap();
    let tied = store
        .options(
            &operator,
            query(OptionKind::Sandbox),
            OwnerContextAccess::DirectoryRead,
        )
        .await
        .unwrap();
    for item in &tied.items {
        assert!(item
            .disambiguator
            .as_deref()
            .unwrap()
            .starts_with("Alice · Created 1970-01-01 00:16:40 UTC · reference "));
    }
    assert_ne!(tied.items[0].disambiguator, tied.items[1].disambiguator);
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    store.db.drop().await.unwrap();
}
