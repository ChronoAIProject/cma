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
    for count in [400, 20_001] {
        let store = fixture().await;
        let states: Vec<_> = (0..count).map(|n| state(n, now() - 60)).collect();
        seed(&store, &states).await;
        store
            .collection("users")
            .insert_many([
                doc! {"_id":"owner-0","display_name":"Alex Chen"},
                doc! {"_id":"owner-1","display_name":"Alex Chen"},
                doc! {"_id":"owner-2","display_name":"Priya [Research]"},
            ])
            .await
            .unwrap();
        store.collection("sandboxes").insert_many([
            doc! {"_id":"runtime-a","record":{"runtime":{"runtime_version":"runtime-stable"}}},
            doc! {"_id":"runtime-b","record":{"lifecycle":{"suspended_runtime":{"runtime_version":"runtime-previous"}}}},
        ]).await.unwrap();
        let operator = scope("operator", true);
        let first = store
            .options(&operator, query(OptionKind::Sandbox))
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
        let second = store
            .options(&operator, second_query.clone())
            .await
            .unwrap();
        assert_eq!(second.items.len(), 50);
        assert!(first
            .items
            .iter()
            .all(|item| second.items.iter().all(|other| item.value != other.value)));
        assert!(store
            .options(&scope("other-operator", true), second_query.clone())
            .await
            .is_err());
        assert!(store
            .options(
                &operator,
                OptionQuery {
                    search: "Research".into(),
                    ..second_query.clone()
                }
            )
            .await
            .is_err());
        assert!(store
            .options(
                &operator,
                OptionQuery {
                    owner: Some("owner-0".into()),
                    ..second_query
                }
            )
            .await
            .is_err());
        assert!(matches!(
            store
                .options(&scope("owner-0", false), query(OptionKind::Owner))
                .await,
            Err(MetricsError::Forbidden)
        ));
        let target = count - 1;
        let found = store
            .options(
                &operator,
                OptionQuery {
                    search: format!("Sandbox {target:06}"),
                    ..query(OptionKind::Sandbox)
                },
            )
            .await
            .unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].value, states[target].sandbox_id.as_str());
        let narrowed = store
            .options(
                &operator,
                OptionQuery {
                    owner: Some("owner-0".into()),
                    ..query(OptionKind::Sandbox)
                },
            )
            .await
            .unwrap();
        assert!(narrowed.items.iter().all(|item| states
            .iter()
            .any(|s| s.sandbox_id.as_str() == item.value && s.owner.as_str() == "owner-0")));
        let owners = store
            .options(
                &operator,
                OptionQuery {
                    search: "alex".into(),
                    ..query(OptionKind::Owner)
                },
            )
            .await
            .unwrap();
        assert_eq!(owners.items.len(), 2);
        assert_ne!(owners.items[0].label, owners.items[1].label);
        assert!(owners
            .items
            .iter()
            .all(|item| item.label.starts_with("Alex Chen · account ")));
        let literal = store
            .options(
                &operator,
                OptionQuery {
                    search: "[Research]".into(),
                    ..query(OptionKind::Owner)
                },
            )
            .await
            .unwrap();
        assert_eq!(literal.items.len(), 1);
        let unnamed = store
            .options(
                &operator,
                OptionQuery {
                    selected: Some("owner-3".into()),
                    ..query(OptionKind::Owner)
                },
            )
            .await
            .unwrap();
        assert!(unnamed.items[0]
            .label
            .starts_with("Unnamed owner · account "));
        let runtimes = store
            .options(&operator, query(OptionKind::Runtime))
            .await
            .unwrap();
        assert_eq!(
            runtimes
                .items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            vec!["runtime-previous", "runtime-stable"]
        );
        let empty = store
            .options(
                &operator,
                OptionQuery {
                    search: "absent".into(),
                    ..query(OptionKind::Sandbox)
                },
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
        store.db.drop().await.unwrap();
    }
}
