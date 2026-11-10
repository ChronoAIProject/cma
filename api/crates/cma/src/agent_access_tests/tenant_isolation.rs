use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use futures_util::TryStreamExt;
use layer2_core::resource_access::CredentialContext;
use layer2_core::sandbox::store::SandboxStore;
use mongodb::bson::{doc, Document};
use serde_json::{json, Value};
use tower::ServiceExt;

// Both principals are valid, capable owners. Only the production store may
// decide whether they own the nominated Agent; the authority double cannot.
struct BothOwners;
#[async_trait::async_trait]
impl PersonalCommandAdmissionAuthority for BothOwners {
    async fn personal_command_authority(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(AgentAuthorizationRevision {
            authority_id: "personal:isolation".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
fn context(subject: &str) -> CallerContext {
    let subject = SubjectId::new(subject);
    CallerContext {
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "https://identity.example.test".into(),
            subject: subject.clone(),
            credential_id: "fixture".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: vec![],
    }
}
async fn call(app: &Router, method: &str, path: &str, body: &Value, subject: &str) -> (u16, Value) {
    let context = context(subject);
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("idempotency-key", "isolation")
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            context.caller.subject.clone(),
        ));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "fixture-custody".into(),
        ));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn requests(client: &mongodb::Client, name: &str) -> Vec<Document> {
    client
        .database(name)
        .collection::<Document>(crate::store::agent_access_requests::COLLECTION)
        .find(doc! {})
        .sort(doc! {"_id": 1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn tenant_isolation_service_access_routes_check_owner_before_receipt_replay() {
    for action in ["list", "approve", "deny", "scope"] {
        let (client, name, store, first) = if action == "scope" {
            crate::store::agent_access_requests::tests::approved_scoped_fixture().await
        } else {
            crate::store::agent_access_requests::tests::approved_fixture().await
        };
        store
            .fail_access_request(first.lease.as_ref().unwrap(), "fixture_complete")
            .await
            .unwrap();
        let mut proposal = first.proposal.clone();
        proposal.request_key = "isolation-proposal".into();
        let record = if action == "scope" {
            first.clone()
        } else {
            store
                .request_agent_access(first.requester.clone(), proposal)
                .await
                .unwrap()
        };
        let current = store
            .get_sandbox(&record.requester.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let expected_services: Vec<_> = current
            .credential
            .effective_reaches()
            .iter()
            .map(ToString::to_string)
            .collect();
        let base = format!("/api/v1/agents/{}", record.requester.agent_id);
        let (method, path, body, expected) = match action {
            "list" => ("GET", format!("{base}/access-requests"), Value::Null, 200),
            "approve" => (
                "POST",
                format!("{base}/access-requests/{}/approve", record.id),
                json!({"services":["github"]}),
                200,
            ),
            "deny" => (
                "POST",
                format!("{base}/access-requests/{}/deny", record.id),
                json!({}),
                200,
            ),
            "scope" => (
                "PUT",
                format!("{base}/service-access"),
                json!({"services":["github"],"expected_services":expected_services}),
                202,
            ),
            _ => unreachable!(),
        };
        let service = AgentAccessService::new(
            store.clone(),
            Arc::new(BothOwners),
            Arc::new(SandboxPolicy {
                baseline_services: vec![],
                ..Default::default()
            }),
        );
        let app = layer1_public::rest::agent_access_requests_router(service);
        let before = requests(&client, &name).await;
        let denied = call(&app, method, &path, &body, "foreign").await;
        assert_eq!(denied.0, 404, "{action}");
        assert_eq!(denied.1["code"], "agent_access_not_found");
        assert_eq!(requests(&client, &name).await, before);
        assert_eq!(
            store.get_sandbox(&current.id).await.unwrap(),
            Some(current.clone())
        );
        let owned = call(&app, method, &path, &body, record.requester.owner.as_str()).await;
        assert_eq!(owned.0, expected, "owner {action}: {}", owned.1["code"]);
        let after = requests(&client, &name).await;
        let after_sandbox = store.get_sandbox(&current.id).await.unwrap();
        assert_eq!(
            call(&app, method, &path, &body, "foreign").await,
            denied,
            "foreign receipt replay"
        );
        assert_eq!(requests(&client, &name).await, after);
        assert_eq!(store.get_sandbox(&current.id).await.unwrap(), after_sandbox);
        client.database(&name).drop().await.unwrap();
    }
}
