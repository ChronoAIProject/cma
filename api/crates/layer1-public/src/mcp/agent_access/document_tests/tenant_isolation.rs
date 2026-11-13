use super::*;
use layer2_core::document_artifacts::DocumentStore;

struct TenantAuth {
    runtime: RuntimeAuth,
    foreign_owner: bool,
    other_agent: bool,
}
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for TenantAuth {
    async fn authenticate_setup(&self, value: &str) -> Result<(), AgentAccessError> {
        self.runtime.authenticate_setup(value).await
    }
    async fn authenticate(&self, value: &str) -> Result<AgentAccessRequester, AgentAccessError> {
        let mut identity = self.runtime.authenticate(value).await?;
        if self.other_agent {
            identity.agent_id = AgentId(format!("agt_{}", "b".repeat(32)));
            identity.creation.agent_id = identity.agent_id.clone();
        }
        if self.foreign_owner {
            identity.owner = SubjectId::new("foreign");
            identity.creation.owner = layer2_core::resource_access::ResourceOwner::Person {
                subject: identity.owner.clone(),
            };
        }
        Ok(identity)
    }
}

#[tokio::test]
async fn tenant_isolation_runtime_document_ids_and_old_receipts_never_expand_authority() {
    let stub = Stub::new(true);
    let rows = Arc::new(MemoryDocuments::default());
    let service = DocumentService::new(rows.clone(), Arc::new(MemoryDocumentBlobs::default()));
    let app = agent_tools_router(
        Arc::new(RuntimeAuth(stub.clone())),
        stub.clone(),
        Some(service.clone()),
        "https://cma.example",
    );
    let created = body(
        app.clone()
            .oneshot(request(&stub, &publish("private-runtime-source".into())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(created["result"]["isError"], false);
    let id = created["result"]["structuredContent"]["document"]["id"]
        .as_str()
        .unwrap();
    let update = |id: &str| {
        rpc(
            "tools/call",
            json!({"name":"update_document","arguments":{
                "request_key":"retained-update", "document_id":id, "expected_revision":1, "title":"Allowed update"
            }}),
        )
    };
    let accepted = body(
        app.clone()
            .oneshot(request(&stub, &update(id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(accepted["result"]["isError"], false);
    let before = serde_json::to_value(rows.get(id).await.unwrap().unwrap()).unwrap();
    for (foreign_owner, other_agent) in [(true, true), (false, true)] {
        let denied_app = agent_tools_router(
            Arc::new(TenantAuth {
                runtime: RuntimeAuth(stub.clone()),
                foreign_owner,
                other_agent,
            }),
            stub.clone(),
            Some(service.clone()),
            "https://cma.example",
        );
        let mut previous = None;
        for target in [id, "doc_00000000000000000000000000000000"] {
            let response = denied_app
                .clone()
                .oneshot(request(&stub, &update(target)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let value = body(response).await;
            assert_eq!(value["result"]["isError"], true);
            assert!(value["result"].get("structuredContent").is_none());
            assert!(!value.to_string().contains("private-runtime-source"));
            if let Some(expected) = &previous {
                assert_eq!(expected, &value);
            }
            previous = Some(value);
        }
        assert_eq!(
            serde_json::to_value(rows.get(id).await.unwrap().unwrap()).unwrap(),
            before
        );
    }
    // The real owner can recover the original receipt, then a stale runtime
    // capability is refused even though that exact receipt still exists.
    assert_eq!(
        body(
            app.clone()
                .oneshot(request(&stub, &update(id)))
                .await
                .unwrap()
        )
        .await,
        accepted
    );
    stub.confirmed.store(false, Ordering::SeqCst);
    let denied = app.oneshot(request(&stub, &update(id))).await.unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::to_value(rows.get(id).await.unwrap().unwrap()).unwrap(),
        before
    );
}
