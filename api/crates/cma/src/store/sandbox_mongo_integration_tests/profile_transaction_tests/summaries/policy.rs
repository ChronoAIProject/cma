use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn summary_routes_preserve_visibility_revision_order_and_page_refusals() {
    let (client, name, store, _) = measured_store().await;
    let router = routes(store.clone());
    for kind in KINDS {
        let uri = format!("/api/v1/{kind}-profiles");
        let (status, empty) = get(&router, "owner", &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(empty["items"], json!([]));
        seed(&store, kind, 7, 3, false).await;
        let (profiles, revisions) = collections(&store, kind);
        // Additive fields ignored by the existing source decoder must not be
        // mistaken for a joined revision discriminator.
        profiles
            .update_one(
                doc! {"_id": id(kind, 0)},
                doc! {"$set": {"profile_id": "additive-legacy-field"}},
            )
            .await
            .unwrap();
        revisions
            .delete_many(doc! { "profile_id": id(kind, 0) })
            .await
            .unwrap();
        revisions
            .delete_many(doc! { "profile_id": id(kind, 1), "revision": { "$gt": 1_i64 } })
            .await
            .unwrap();
        // Published time and stale summary hints must never replace numeric revision order.
        revisions
            .update_one(
                doc! { "profile_id": id(kind, 2), "revision": 3_i64 },
                doc! { "$set": { "record.published_at": "2020-01-01T00:00:00Z" } },
            )
            .await
            .unwrap();
        profiles.update_one(doc! { "_id": id(kind, 2) }, doc! { "$set": { "visibility": "public", "record.visibility": "public", "last_revision": 1_i64, "latest_published_at": "2030-01-01T00:00:00Z" } }).await.unwrap();
        profiles
            .update_one(
                doc! { "_id": id(kind, 3) },
                doc! { "$set": { "retired_at": NOW, "record.retired_at": NOW } },
            )
            .await
            .unwrap();
        profiles
            .update_one(
                doc! { "_id": id(kind, 4) },
                doc! { "$set": { "record.internal": true } },
            )
            .await
            .unwrap();
        profiles
            .update_many(
                doc! { "_id": { "$in": [id(kind, 5), id(kind, 6)] } },
                doc! { "$set": { "owner": "foreign", "record.owner": "foreign" } },
            )
            .await
            .unwrap();
        profiles
            .update_one(
                doc! { "_id": id(kind, 6) },
                doc! { "$set": { "visibility": "public", "record.visibility": "public" } },
            )
            .await
            .unwrap();
        let (status, page) = get(&router, "owner", &uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["items"].as_array().unwrap().len(), 4);
        for (n, item) in page["items"].as_array().unwrap().iter().enumerate() {
            assert_eq!(item["id"], id(kind, n));
            assert_eq!(
                *item,
                get(&router, "owner", &format!("{uri}/{}", id(kind, n)))
                    .await
                    .1
            );
        }
        assert!(page["items"][0]["latest_revision"].is_null());
        assert_eq!(page["items"][1]["latest_revision"]["revision"], 1);
        assert_eq!(page["items"][2]["latest_revision"]["revision"], 3);
        assert_eq!(
            page["items"][2]["latest_revision"]["published_at"],
            "2020-01-01T00:00:00Z"
        );
        assert_eq!(page["items"][3]["retired_at"], NOW);
        assert_eq!(get(&router, "unknown", &uri).await.1["items"], json!([]));
        let foreign = get(&router, "foreign", &uri).await.1;
        assert_eq!(foreign["items"].as_array().unwrap().len(), 2);
        assert!(foreign["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["owner"] == "foreign"));
        for query in ["limit=0", "limit=201", "limit=bad"] {
            assert_eq!(
                get(&router, "owner", &format!("{uri}?{query}")).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        let mut denied = Request::builder().uri(&uri).body(Body::empty()).unwrap();
        denied
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(SubjectId(
                "owner".into(),
            )));
        denied.extensions_mut().insert(Authorization::new([]));
        assert_eq!(
            router.clone().oneshot(denied).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn summary_routes_keep_revision_and_candidate_corruption_checks() {
    let (client, name, store, _) = measured_store().await;
    let router = routes(store.clone());
    for kind in KINDS {
        seed(&store, kind, 3, 4, false).await;
        let (profiles, revisions) = collections(&store, kind);
        let uri = format!("/api/v1/{kind}-profiles?limit=2");
        // Neither the old latest read nor the batch reader traverses older history.
        revisions
            .update_one(
                doc! { "profile_id": id(kind,0), "revision": 1_i64 },
                doc! { "$set": { "record.digest": "corrupt" } },
            )
            .await
            .unwrap();
        // The extra profile is only a cursor lookahead; its revisions were never read.
        revisions
            .update_one(
                doc! { "profile_id": id(kind,2), "revision": 4_i64 },
                doc! { "$set": { "record.digest": "corrupt", "profile_id": [id(kind, 2)] } },
            )
            .await
            .unwrap();
        assert_eq!(get(&router, "owner", &uri).await.0, StatusCode::OK);
        for revision in [3_i64, 4] {
            let filter = doc! { "profile_id": id(kind,0), "revision": revision };
            let original = revisions.find_one(filter.clone()).await.unwrap().unwrap();
            revisions
                .update_one(
                    filter.clone(),
                    doc! { "$set": { "record.digest": "corrupt" } },
                )
                .await
                .unwrap();
            let (status, body) = get(&router, "owner", &uri).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert!(body.get("items").is_none());
            assert_eq!(
                get(
                    &router,
                    "owner",
                    &format!("/api/v1/{kind}-profiles/{}", id(kind, 0))
                )
                .await
                .0,
                StatusCode::SERVICE_UNAVAILABLE
            );
            revisions.replace_one(filter, original).await.unwrap();
        }
        // Query equality includes an array containing the requested key. The
        // old reader rejects that malformed source; the join must not hide it.
        let filter = doc! { "profile_id": id(kind, 0), "revision": 4_i64 };
        let original = revisions.find_one(filter.clone()).await.unwrap().unwrap();
        revisions
            .update_one(
                filter.clone(),
                doc! { "$set": { "profile_id": [id(kind, 0)] } },
            )
            .await
            .unwrap();
        assert_eq!(
            get(&router, "owner", &uri).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            get(
                &router,
                "owner",
                &format!("/api/v1/{kind}-profiles/{}", id(kind, 0))
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        revisions.replace_one(filter, original).await.unwrap();
        // Preserve validation of the 201st (here third) candidate itself.
        profiles
            .update_one(
                doc! { "_id": id(kind,2) },
                doc! { "$set": { "record.owner": "wrong-owner" } },
            )
            .await
            .unwrap();
        let (status, body) = get(&router, "owner", &uri).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body.get("items").is_none());
    }
    client.database(&name).drop().await.unwrap();
}
