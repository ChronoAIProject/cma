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

struct ShareDirectory;
#[async_trait::async_trait]
impl layer2_core::UserDirectoryStore for ShareDirectory {
    async fn observe(
        &self,
        _: &layer2_core::PublicIdentity,
    ) -> Result<(), layer2_core::StoreError> {
        Ok(())
    }
    async fn user(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<layer2_core::RegisteredUser>, layer2_core::StoreError> {
        Ok(["owner-a", "recipient", "outsider"]
            .contains(&subject.as_str())
            .then(|| layer2_core::RegisteredUser {
                identity: layer2_core::PublicIdentity {
                    subject: subject.clone(),
                    display_name: Some(format!("Name {subject}")),
                    avatar_url: None,
                },
                first_seen_at: "2026-09-05T00:00:00Z".into(),
                last_seen_at: "2026-09-05T00:00:00Z".into(),
            }))
    }
    async fn list(
        &self,
        query: &layer2_core::DirectoryQuery,
    ) -> Result<layer2_core::Page<layer2_core::RegisteredUser>, layer2_core::StoreError> {
        let mut items = Vec::new();
        for subject in ["owner-a", "recipient", "outsider"] {
            if query
                .text
                .as_ref()
                .is_none_or(|text| format!("Name {subject}").contains(text))
            {
                items.push(self.user(&SubjectId::new(subject)).await?.unwrap());
            }
        }
        Ok(layer2_core::Page {
            items,
            next_cursor: None,
        })
    }
}
pub(super) struct Authority {
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
pub(super) async fn app() -> (Router, Arc<InMemorySandboxStore>, Arc<Authority>) {
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
        ResourceAccessService::new(fixture.store.clone(), authority.clone())
            .with_directory(layer2_core::UserDirectory::new(Arc::new(ShareDirectory))),
    ));
    (router, fixture.store, authority)
}
pub(super) fn request(
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

const LINKS: &str = "/api/v1/agents/agt_share/share-links";

#[tokio::test]
async fn agent_link_requires_secret_and_redemption_and_revokes_one_link_only() {
    let (router, store, _) = app().await;
    let token = "ab".repeat(32);
    let (status, created) = json_response(
        &router,
        request(
            "POST",
            LINKS,
            "owner-a",
            Some("link-view"),
            Some(json!({"audience":{"kind":"public"},"mode":"view","token":token})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert!(!created.to_string().contains(&token));
    assert!(created.get("token_sha256").is_none());
    let id = created["id"].as_str().unwrap();
    let durable = store.resource_share(id).await.unwrap().unwrap();
    assert_ne!(durable.link.as_ref().unwrap().token_sha256, token);
    assert!(!serde_json::to_string(&durable).unwrap().contains(&token));
    let redeem = format!("/api/v1/agent-share-links/{id}/redeem");
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "outsider", None, None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                &redeem,
                "outsider",
                None,
                Some(json!({"token":"ff".repeat(32)}))
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, accepted) = json_response(
        &router,
        request(
            "POST",
            &redeem,
            "outsider",
            None,
            Some(json!({"token":token})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["accepted_count"], 1);
    let (status, access) =
        json_response(&router, request("GET", ACCESS, "outsider", None, None)).await;
    assert_eq!(status, StatusCode::OK, "{access}");
    let caps = access["capabilities"].as_array().unwrap();
    assert!(caps.contains(&json!("view_files")));
    assert!(caps.contains(&json!("view_history")));
    for denied in [
        "interact",
        "operate_agent",
        "manage_sharing",
        "manage_sandbox",
    ] {
        assert!(!caps.contains(&json!(denied)));
    }
    assert_eq!(access["shared_by"]["subject"], "owner-a");
    let (status, replay) = json_response(
        &router,
        request(
            "POST",
            &redeem,
            "outsider",
            None,
            Some(json!({"token":token})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["revision"], replay["revision"]);
    let (status, second) = json_response(
        &router,
        request(
            "POST",
            LINKS,
            "owner-a",
            Some("link-write"),
            Some(json!({"audience":{"kind":"public"},"mode":"view_write","token":"cd".repeat(32)})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let second_id = second["id"].as_str().unwrap();
    let (status, second) = json_response(
        &router,
        request(
            "POST",
            &format!("/api/v1/agent-share-links/{second_id}/redeem"),
            "outsider",
            None,
            Some(json!({"token":"cd".repeat(32)})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let revoke_path = format!("{LINKS}/{id}?expected_revision={}", accepted["revision"]);
    let (status, revoked) = json_response(
        &router,
        request("DELETE", &revoke_path, "owner-a", Some("revoke-view"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                &redeem,
                "outsider",
                None,
                Some(json!({"token":token}))
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, access) =
        json_response(&router, request("GET", ACCESS, "outsider", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let caps = access["capabilities"].as_array().unwrap();
    assert!(caps.contains(&json!("interact")));
    assert!(caps.contains(&json!("operate_agent")));
    assert!(!caps.contains(&json!("manage_sandbox")));
    assert!(!caps.contains(&json!("manage_sharing")));
    let target = ResourceTarget::Agent {
        agent_id: layer2_core::AgentId("agt_share".into()),
    };
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(snapshot.shares.len(), 1);
    assert!(snapshot
        .shared_agent(&SubjectId::new("outsider"), &[])
        .is_ok());
    let final_path = format!(
        "{LINKS}/{second_id}?expected_revision={}",
        second["revision"]
    );
    assert_eq!(
        json_response(
            &router,
            request("DELETE", &final_path, "owner-a", Some("revoke-write"), None)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "outsider", None, None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .shared_agent(&SubjectId::new("outsider"), &[])
        .is_err());
}

#[tokio::test]
async fn agent_link_organization_is_revalidated_after_redemption() {
    let (router, _, authority) = app().await;
    let token = "01".repeat(32);
    let (status, created) = json_response(&router, request("POST", LINKS, "owner-a", Some("org-link"), Some(json!({"audience":{"kind":"organization","organization_id":"org"},"mode":"view_write","token":token})))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let redeem = format!(
        "/api/v1/agent-share-links/{}/redeem",
        created["id"].as_str().unwrap()
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                &redeem,
                "outsider",
                None,
                Some(json!({"token":token}))
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
                "POST",
                &redeem,
                "recipient",
                None,
                Some(json!({"token":token}))
            )
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "recipient", None, None))
            .await
            .0,
        StatusCode::OK
    );
    authority.member.store(false, Ordering::SeqCst);
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "recipient", None, None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn agent_link_users_require_canonical_selection_and_cannot_be_redeemed_by_others() {
    let (router, _, _) = app().await;
    let token = "23".repeat(32);
    let (status, response) = json_response(
        &router,
        request(
            "GET",
            "/api/v1/agents/agt_share/share-recipients?q=recipient",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["items"][0]["subject"], "recipient");
    assert_eq!(
        json_response(
            &router,
            request(
                "GET",
                "/api/v1/agents/agt_share/share-recipients?q=recipient",
                "outsider",
                None,
                None
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let body = json!({"audience":{"kind":"users","subjects":["unknown-user"]},"mode":"view_write","token":token});
    assert_eq!(
        json_response(
            &router,
            request("POST", LINKS, "owner-a", Some("unknown-link"), Some(body))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let body = json!({"audience":{"kind":"users","subjects":["recipient"]},"mode":"view_write","token":token});
    let (status, created) = json_response(
        &router,
        request("POST", LINKS, "owner-a", Some("users-link"), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let redeem = format!(
        "/api/v1/agent-share-links/{}/redeem",
        created["id"].as_str().unwrap()
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                &redeem,
                "outsider",
                None,
                Some(json!({"token":token}))
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        json_response(
            &router,
            request(
                "POST",
                &redeem,
                "recipient",
                None,
                Some(json!({"token":token}))
            )
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        json_response(&router, request("GET", ACCESS, "recipient", None, None))
            .await
            .0,
        StatusCode::OK
    );
}
