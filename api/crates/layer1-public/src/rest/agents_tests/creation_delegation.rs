use super::*;
use layer2_core::resource_access::{
    CallerContext, CredentialContext, CredentialSource, ResourceActor,
};
use layer2_core::sandbox::store::AgentProvenanceStore;
use layer2_core::Caller;

fn delegated() -> CallerContext {
    CallerContext {
        caller: Caller::new(
            SubjectId::new(OWNER),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "fixture".into(),
            subject: SubjectId::new(OWNER),
            credential_id: "verified-credential".into(),
        },
        credential_source: CredentialSource::delegated("actor-grant"),
        actor: ResourceActor::Api {
            subject: SubjectId::new(OWNER),
            delegation_id: Some("actor-grant".into()),
        },
        memberships: vec![],
    }
}

#[tokio::test]
async fn create_retains_verified_delegation_and_rejects_forged_grant_without_effects() {
    let fixture = fixture().await;
    let context = delegated();
    let path = format!("/api/v1/workspaces/{}/agents", fixture.workspace_id);
    let body = json!({
        "agent_profile": {"id":"agp_route", "revision":1},
        "title":"Delegated agent",
        "first_message":"Continue the user's task."
    });
    let make = |context: CallerContext| {
        let mut incoming = request(
            Method::POST,
            &path,
            OWNER,
            Some("delegated-create"),
            Some(body.clone()),
        );
        incoming.extensions_mut().insert(context);
        incoming
    };
    let mut forged = context.clone();
    forged.credential_source = CredentialSource::Retained;
    assert_eq!(
        response(&fixture.router, make(forged)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(fixture
        .store
        .list_agents(&SubjectId::new(OWNER), &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let created = response(&fixture.router, make(context.clone())).await;
    assert_eq!(created.0, StatusCode::CREATED);
    let replay = response(&fixture.router, make(context.clone())).await;
    assert_eq!(created, replay);
    let stored = fixture
        .store
        .list_agents(&SubjectId::new(OWNER), &Default::default())
        .await
        .unwrap()
        .items;
    assert_eq!(stored.len(), 1);
    assert_eq!(
        fixture
            .store
            .agent_provenance_snapshot(&stored[0].id)
            .await
            .unwrap()
            .creation
            .unwrap()
            .created_by,
        Some(context.actor)
    );
}
