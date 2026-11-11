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
async fn metrics_options_search_page_and_resolve_at_scale_without_reports() {
    let mut window = super::circuit_capture::ScaleWindow::start_for("options", 0);
    for count in [400, 20_001] {
        window.cohort(count);
        let store = fixture().await;
        window.attach(&store);
        window.phase("seed_states", 0);
        let states: Vec<_> = (0..count)
            .map(|n| {
                let mut row = state(n, now() - 60);
                row.owner = SubjectId(format!("owner-{}", n % 120));
                row
            })
            .collect();
        // Match the other scale fixtures: setup must not hold a reporting
        // connection for one large insert before option queries begin.
        for (batch, states) in states.chunks(200).enumerate() {
            window.phase("seed_states", batch + 1);
            seed(&store, states).await;
        }
        let users: Vec<_> = (0..120).filter(|n| *n != 3).map(|n| {
            let name = match n { 0 | 1 => "Alex Chen".into(), 2 => "Priya [Research]".into(), _ => format!("Operator {n}") };
            doc! {"_id":format!("owner-{n}"),"display_name":name,"first_seen_at":format!("2026-11-{:02}T12:30:00Z",n%28+1)}
        }).collect();
        window.phase("seed_users", 0);
        store.collection("users").insert_many(users).await.unwrap();
        let inventory: Vec<_> = states.iter().enumerate().map(|(n, row)| {
            if n % 2 == 0 {
                doc! {"_id":row.sandbox_id.as_str(),"record":{"runtime":{"runtime_version":"runtime-stable"}}}
            } else {
                doc! {"_id":row.sandbox_id.as_str(),"record":{"lifecycle":{"suspended_runtime":{"runtime_version":"runtime-previous"}}}}
            }
        }).collect();
        for (batch, inventory) in inventory.chunks(200).enumerate() {
            window.phase("seed_inventory", batch + 1);
            store
                .collection("sandboxes")
                .insert_many(inventory)
                .await
                .unwrap();
        }
        window.phase("verify_seed_cohort", 0);
        for collection in [super::super::super::sandbox_usage::STATES, "sandboxes"] {
            assert_eq!(
                store
                    .collection(collection)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                count as u64,
            );
        }
        let operator = scope("operator", true);
        let owner_started = std::time::Instant::now();
        let mut owner_ids = std::collections::BTreeSet::new();
        let mut owner_query = query(OptionKind::Owner);
        let mut owner_pages = 0;
        loop {
            window.phase("owner_page", owner_pages + 1);
            let page = tokio::time::timeout(
                Duration::from_secs(5),
                store.options(
                    &operator,
                    owner_query.clone(),
                    OwnerContextAccess::DirectoryRead,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            owner_pages += 1;
            assert!(page.items.len() <= 50);
            for item in page.items {
                assert!(owner_ids.insert(item.value));
            }
            if let Some(next) = page.next {
                owner_query.cursor = Some(next);
            } else {
                break;
            }
        }
        assert_eq!(owner_ids.len(), 120);
        assert_eq!(owner_pages, 3);
        let owner_elapsed = owner_started.elapsed();
        window.phase("sandbox_page", 1);
        let first = store
            .options(
                &operator,
                query(OptionKind::Sandbox),
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(first.items.len(), 50);
        assert!(first.next.is_some());
        assert!(first
            .items
            .iter()
            .all(|item| item.label.starts_with("Sandbox ") && !item.label.contains(&item.value)));
        let second_query = OptionQuery {
            cursor: first.next.clone(),
            ..query(OptionKind::Sandbox)
        };
        window.phase("sandbox_page", 2);
        let second = store
            .options(
                &operator,
                second_query.clone(),
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(second.items.len(), 50);
        assert!(first.items.iter().all(|item| item.disambiguator.is_none()));
        assert!(first
            .items
            .iter()
            .all(|item| second.items.iter().all(|other| item.value != other.value)));
        assert!(store
            .options(
                &scope("other-operator", true),
                second_query.clone(),
                OwnerContextAccess::DirectoryRead
            )
            .await
            .is_err());
        assert!(store
            .options(
                &operator,
                OptionQuery {
                    search: "Research".into(),
                    ..second_query.clone()
                },
                OwnerContextAccess::DirectoryRead
            )
            .await
            .is_err());
        assert!(store
            .options(
                &operator,
                OptionQuery {
                    owner: Some("owner-0".into()),
                    ..second_query
                },
                OwnerContextAccess::DirectoryRead
            )
            .await
            .is_err());
        assert!(matches!(
            store
                .options(
                    &scope("owner-0", false),
                    query(OptionKind::Owner),
                    OwnerContextAccess::DirectoryRead
                )
                .await,
            Err(MetricsError::Forbidden)
        ));
        let target = count - 1;
        window.phase("sandbox_search", 0);
        let found = store
            .options(
                &operator,
                OptionQuery {
                    search: format!("Sandbox {target:06}"),
                    ..query(OptionKind::Sandbox)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].value, states[target].sandbox_id.as_str());
        window.phase("sandbox_owner_filter", 0);
        let narrowed = store
            .options(
                &operator,
                OptionQuery {
                    owner: Some("owner-0".into()),
                    ..query(OptionKind::Sandbox)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert!(narrowed.items.iter().all(|item| states
            .iter()
            .any(|s| s.sandbox_id.as_str() == item.value && s.owner.as_str() == "owner-0")));
        window.phase("owner_search", 0);
        let owners = store
            .options(
                &operator,
                OptionQuery {
                    search: "alex".into(),
                    ..query(OptionKind::Owner)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(owners.items.len(), 2);
        assert_eq!(owners.items[0].label, owners.items[1].label);
        assert_eq!(
            owners.items[0].disambiguator.as_deref(),
            Some("Joined 2026-11-01")
        );
        assert_eq!(
            owners.items[1].disambiguator.as_deref(),
            Some("Joined 2026-11-02")
        );
        assert!(owners.items.iter().all(|item| item.label == "Alex Chen"));
        window.phase("owner_literal_search", 0);
        let literal = store
            .options(
                &operator,
                OptionQuery {
                    search: "[Research]".into(),
                    ..query(OptionKind::Owner)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(literal.items.len(), 1);
        window.phase("owner_resolve", 0);
        let unnamed = store
            .options(
                &operator,
                OptionQuery {
                    selected: Some("owner-3".into()),
                    ..query(OptionKind::Owner)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(unnamed.items[0].label, "Unnamed owner");
        assert!(unnamed.items[0].disambiguator.is_none());
        window.phase("seed_duplicate_labels", 0);
        for (n, owner) in [(0, "owner-0"), (1, "owner-2")] {
            store
                .collection(super::super::super::sandbox_usage::STATES)
                .update_one(
                    doc! {"_id":states[n].sandbox_id.as_str()},
                    doc! {"$set":{"record.name":"Shared research","owner":owner}},
                )
                .await
                .unwrap();
        }
        window.phase("sandbox_duplicate_search", 0);
        let duplicates = store
            .options(
                &operator,
                OptionQuery {
                    search: "Shared research".into(),
                    ..query(OptionKind::Sandbox)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(duplicates.items.len(), 2);
        assert_eq!(duplicates.items[0].label, "Shared research");
        assert_eq!(
            duplicates.items[0].disambiguator.as_deref(),
            Some("Alex Chen")
        );
        assert_eq!(
            duplicates.items[1].disambiguator.as_deref(),
            Some("Priya [Research]")
        );
        window.phase("sandbox_resolve", 0);
        let only = store
            .options(
                &operator,
                OptionQuery {
                    selected: Some(duplicates.items[0].value.clone()),
                    ..query(OptionKind::Sandbox)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert_eq!(only.items[0].label, "Shared research");
        assert!(only.items[0].disambiguator.is_none());
        window.phase("runtime_discovery", 0);
        let runtime_started = std::time::Instant::now();
        let runtimes = tokio::time::timeout(
            Duration::from_secs(5),
            store.options(
                &operator,
                query(OptionKind::Runtime),
                OwnerContextAccess::DirectoryRead,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        println!("option scale: sandboxes={count}, owners={}, owner_pages={owner_pages}, owner_ms={}, runtime_ms={}", owner_ids.len(), owner_elapsed.as_millis(), runtime_started.elapsed().as_millis());
        assert_eq!(
            runtimes
                .items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            vec!["runtime-previous", "runtime-stable"]
        );
        window.phase("seed_identifier_labels", 0);
        let identifier_name = "Research-91892afa-5fa5-4f4f-9100-2b8d986d2f71";
        store
            .collection(super::super::super::sandbox_usage::STATES)
            .update_one(
                doc! {"_id":states[0].sandbox_id.as_str()},
                doc! {"$set":{"record.name":identifier_name}},
            )
            .await
            .unwrap();
        store.collection("sandboxes").insert_one(doc! {"_id":"uuid-runtime", "record":{"runtime":{"runtime_version":identifier_name}}}).await.unwrap();
        window.phase("identifier_label_resolution", 0);
        for (kind, value, label) in [
            (
                OptionKind::Sandbox,
                states[0].sandbox_id.as_str(),
                "Unnamed Sandbox",
            ),
            (OptionKind::Runtime, identifier_name, "Unnamed runtime"),
        ] {
            let selected = store
                .options(
                    &operator,
                    OptionQuery {
                        selected: Some(value.into()),
                        ..query(kind)
                    },
                    OwnerContextAccess::DirectoryRead,
                )
                .await
                .unwrap();
            assert_eq!(selected.items[0].value, value);
            assert_eq!(selected.items[0].label, label);
            assert!(selected.items[0].disambiguator.is_none());
        }
        let empty = store
            .options(
                &operator,
                OptionQuery {
                    search: "absent".into(),
                    ..query(OptionKind::Sandbox)
                },
                OwnerContextAccess::DirectoryRead,
            )
            .await
            .unwrap();
        assert!(empty.items.is_empty());
        assert_eq!(
            store
                .collection(JOBS)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        window.phase("cleanup", 0);
        store.db.drop().await.unwrap();
        window.phase("done", 0);
    }
}
