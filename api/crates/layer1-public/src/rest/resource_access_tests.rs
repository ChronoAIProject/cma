use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use layer2_core::resource_access::*;
use layer2_core::sandbox::{store::InMemorySandboxStore, AgentStatus};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::SystemTime,
};

use super::agents_tests::{fixture, insert_agent, json_response, seeded_agent};
use crate::{AuthenticatedSubject, DelegatedCredential};

struct Authority {
    member: AtomicBool,
}
pub(super) fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: format!("credential-{subject}"),
        },
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        memberships: Vec::new(),
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "request-credential");
        if caller.subject.as_str() == "outsider" || !self.member.load(Ordering::SeqCst) {
            return Err(MembershipError::NotMember);
        }
        let mut value = context(caller.subject.as_str());
        let now = SystemTime::now();
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            OrganizationRole::Admin,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}
async fn app() -> (Router, Arc<InMemorySandboxStore>, Arc<Authority>) {
    let fixture = fixture().await;
    insert_agent(
        &fixture.store,
        seeded_agent("agt_share", AgentStatus::Ready, 0, None),
        Vec::new(),
    )
    .await;
    let authority = Arc::new(Authority {
        member: AtomicBool::new(true),
    });
    let router = super::resource_access::resource_access_router(Arc::new(
        ResourceAccessService::new(fixture.store.clone(), authority.clone()),
    ));
    (router, fixture.store, authority)
}
fn request(
    method: &str,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    let mut request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let context = context(subject);
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(context.caller.subject.clone()));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("request-credential".into()));
    request
}
const SHARES: &str = "/api/v1/resource-shares/agent/agt_share";
const ACCESS: &str = "/api/v1/resource-access/agent/agt_share";
fn create_body() -> Value {
    json!({"target_org_id":"org", "mode":"view"})
}

#[tokio::test]
async fn owner_crud_replays_original_responses_and_preserves_the_native_agent() {
    let (router, store, _) = app().await;
    let target = ResourceTarget::Agent {
        agent_id: layer2_core::AgentId("agt_share".into()),
    };
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let (status, created) = json_response(
        &router,
        request(
            "POST",
            SHARES,
            "owner-a",
            Some("create"),
            Some(create_body()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["actor"]["subject"], "owner-a");
    assert_eq!(
        created["share"]["owner"],
        json!({"kind":"person","subject":"owner-a"})
    );
    assert!(!created.to_string().contains("request-credential"));
    let path = format!("{SHARES}/{}", created["share"]["id"].as_str().unwrap());
    let (status, updated) = json_response(
        &router,
        request(
            "PATCH",
            &path,
            "owner-a",
            Some("update"),
            Some(json!({"mode":"view","expected_revision":1})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["share"]["revision"], 2);
    let (status, revoked) = json_response(
        &router,
        request(
            "DELETE",
            &format!("{path}?expected_revision=2"),
            "owner-a",
            Some("revoke"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert!(revoked["share"]["revoked_at"].is_string());
    let (status, replay) = json_response(
        &router,
        request(
            "POST",
            SHARES,
            "owner-a",
            Some("create"),
            Some(create_body()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, created);
    let (_, listed) = json_response(&router, request("GET", SHARES, "owner-a", None, None)).await;
    assert_eq!(listed, json!({"items":[]}));
    let (_, read) = json_response(&router, request("GET", &path, "owner-a", None, None)).await;
    assert_eq!(read, revoked["share"]);
    let (status, _) = json_response(
        &router,
        request(
            "POST",
            SHARES,
            "owner-a",
            Some("create"),
            Some(json!({"target_org_id":"other", "mode":"view"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        store
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
}

#[tokio::test]
async fn recipient_effective_access_is_narrow_and_revalidated_without_inventory_leakage() {
    let (router, _, authority) = app().await;
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                SHARES,
                "owner-a",
                Some("create"),
                Some(create_body())
            )
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, access) =
        json_response(&router, request("GET", ACCESS, "recipient", None, None)).await;
    assert_eq!(status, StatusCode::OK, "{access}");
    assert_eq!(
        access["owner"],
        json!({"kind":"person", "is_current_person":false})
    );
    assert_eq!(
        access["capabilities"],
        json!(["view_details", "view_history", "view_artifacts"])
    );
    assert!(access["revalidate_at"].is_string());
    for private in [
        "owner-a",
        "sbx_route",
        "wks_route",
        "conversation-agt_share",
        "request-credential",
    ] {
        assert!(!access.to_string().contains(private));
    }
    assert_eq!(
        json_response(&router, request("GET", SHARES, "recipient", None, None))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                SHARES,
                "recipient",
                Some("reshare"),
                Some(create_body())
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "GET",
                "/api/v1/resource-access/sandbox/sbx_route",
                "recipient",
                None,
                None
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "outsider", None, None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    authority.member.store(false, Ordering::SeqCst);
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "recipient", None, None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, own_access) =
        json_response(&router, request("GET", ACCESS, "owner-a", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        own_access["owner"],
        json!({"kind":"person", "is_current_person":true})
    );
    assert!(!own_access.to_string().contains("owner-a"));
}

#[tokio::test]
async fn effective_organization_owner_is_explicit_without_exposing_parent_metadata() {
    use layer2_core::sandbox::store::SandboxStore;
    let (router, store, _) = app().await;
    let root = store
        .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
        .await
        .unwrap()
        .unwrap();
    store.set_resource_policy_for_test(ResourcePolicyRecord {
        sandbox_id: root.id,
        sandbox_created_at: root.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: root.owner.clone(),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: root.owner,
            admitted_by: context("manager").actor,
            verified_at: root.created_at,
        }),
        revision: 1,
    });
    let (status, access) =
        json_response(&router, request("GET", ACCESS, "recipient", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        access["owner"],
        json!({"kind":"organization", "organization_id":"owner-a"})
    );
    assert!(access["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("manage_sharing")));
    for private in [
        "sbx_route",
        "wks_route",
        "conversation-agt_share",
        "request-credential",
    ] {
        assert!(!access.to_string().contains(private));
    }
}

#[tokio::test]
async fn http_rejects_authority_fields_unverified_extensions_and_unsafe_interaction() {
    let (router, _, _) = app().await;
    for field in [
        "owner",
        "actor",
        "policy_fence",
        "memberships",
        "issuer_actor",
    ] {
        let mut body = create_body();
        body[field] = json!({"subject":"owner-a"});
        assert_eq!(
            json_response(
                &router,
                request("POST", SHARES, "owner-a", Some(field), Some(body))
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        json_response(
            &router,
            request("POST", SHARES, "owner-a", None, Some(create_body()))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, unsafe_share) = json_response(
        &router,
        request(
            "POST",
            SHARES,
            "owner-a",
            Some("unsafe"),
            Some(json!({"target_org_id":"org","mode":"org_roles"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(unsafe_share["code"], "unsafe_interactive_share");
    let mut forged = request("GET", ACCESS, "outsider", None, None);
    forged.extensions_mut().insert(context("owner-a"));
    assert_eq!(
        json_response(&router, forged).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut missing = request("GET", ACCESS, "owner-a", None, None);
    missing.extensions_mut().remove::<CallerContext>();
    assert_eq!(
        json_response(&router, missing).await.0,
        StatusCode::UNAUTHORIZED
    );
}
