use super::*;
use public::request;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_share_batches_preserve_covering_limits_and_refuse_overflow() {
    let f = fixture::Fixture::new(2, false).await;
    let target = ResourceTarget::Agent {
        agent_id: AgentId("agt_list_0000".into()),
    };
    let snapshot = f.store.resource_access_snapshot(&target).await.unwrap();
    f.store
        .mutate_share(mutation(&snapshot, "batch-covering", ShareMode::View))
        .await
        .unwrap();
    let source = f
        .store
        .resource_shares
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    f.store.resource_shares.delete_many(doc! {}).await.unwrap();
    let mut documents = Vec::new();
    for index in 0..MAX_ACTIVE_RESOURCE_SHARES {
        let id = format!("shr_cover_{index:03}");
        let mut document = source.clone();
        document.insert("_id", &id);
        document
            .get_document_mut("record")
            .unwrap()
            .insert("id", id);
        if index % 2 == 0 {
            document.insert("selector_kind", "sandbox");
            document.insert("selector_id", "sbx_list_0000");
            document.get_document_mut("record").unwrap().insert(
                "selector",
                doc! {"kind":"sandbox","sandbox_id":"sbx_list_0000"},
            );
        }
        documents.push(document);
    }
    f.store
        .resource_shares
        .insert_many(documents)
        .await
        .unwrap();
    // Noncovering and revoked projections are excluded before bounding the read.
    for (id, fields) in [
        ("aaa_other_agent", doc! {"selector_id":"other"}),
        ("aaa_other_parent", doc! {"sandbox_id":"other"}),
        ("aaa_revoked", doc! {"revoked":true}),
    ] {
        let mut document = source.clone();
        document.insert("_id", id);
        document.extend(fields);
        f.store.resource_shares.insert_one(document).await.unwrap();
    }
    for path in [
        "/api/v1/agents",
        "/api/v1/agents/provenance",
        "/api/v1/agents/provenance?sandbox=sbx_list_0000",
    ] {
        let expected = request(&f.app(false, false), path).await;
        assert!(expected.0.is_success(), "{path}: {expected:?}");
        f.meter.take();
        assert_eq!(request(&f.app(true, false), path).await, expected);
        assert!(f.meter.take().count() <= 10);
    }
    f.store.resource_shares.insert_one(source).await.unwrap();
    for path in ["/api/v1/agents", "/api/v1/agents/provenance"] {
        let expected = request(&f.app(false, false), path).await;
        assert!(!expected.0.is_success());
        assert_eq!(request(&f.app(true, false), path).await, expected);
    }
    f.close().await;
}
