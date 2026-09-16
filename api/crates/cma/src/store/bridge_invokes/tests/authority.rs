use super::*;
use layer2_core::{Authorization, Capability, SubjectId};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn only_actual_owner_can_issue_and_actor_issuer_or_operator_substitution_refuses() {
    let f = Fixture::new().await;
    for case in 0..5 {
        let mut context = caller();
        match case {
            0 => {
                context.caller.subject = SubjectId::new("foreign");
                context.credential.subject = context.caller.subject.clone();
                context.actor = ResourceActor::Human {
                    subject: context.caller.subject.clone(),
                };
            }
            1 => context.credential.issuer = "https://other.example".into(),
            2 => {
                context.actor = ResourceActor::Api {
                    subject: context.caller.subject.clone(),
                    delegation_id: Some("delegation".into()),
                }
            }
            3 => context.caller.authorization = Authorization::new([Capability::ControlAnySandbox]),
            _ => {
                context.actor = ResourceActor::Human {
                    subject: SubjectId::new("invented-admin"),
                }
            }
        }
        assert!(f
            .service
            .issue(&context, None, f.issue("denied"))
            .await
            .is_err());
    }
    assert_eq!(f.store.grants.count_documents(doc! {}).await.unwrap(), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn organization_admin_consent_retains_actual_actor_and_refuses_members_viewers_and_owner_kind_changes(
) {
    let f = Fixture::organization().await;
    let grant = f
        .service
        .issue(
            &f.context,
            Some("fixture-credential"),
            f.issue("org-consent"),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(
        grant.owner,
        ResourceOwner::Organization {
            subject: SubjectId::new("owner")
        }
    );
    assert_eq!(
        grant.issued_by,
        ResourceActor::Human {
            subject: SubjectId::new("admin")
        }
    );
    for role in [OrganizationRole::Member, OrganizationRole::Viewer] {
        *f.authority.role.lock().unwrap() = Some(role);
        assert_eq!(
            f.service
                .issue(&f.context, Some("fixture-credential"), f.issue("member"))
                .await,
            Err(BridgeInvokeError::Bridge(BridgeError::Forbidden))
        );
        assert_eq!(
            f.service
                .revoke(
                    &f.context,
                    Some("fixture-credential"),
                    f.revoke(&grant, "member-revoke")
                )
                .await,
            Err(BridgeInvokeError::Bridge(BridgeError::Forbidden))
        );
    }
    *f.authority.role.lock().unwrap() = Some(OrganizationRole::Admin);
    let effect = BridgeInvokeEffect::AdmitMessage {
        inbox_id: "org-inbox".into(),
    };
    let proof = f
        .service
        .authorize(f.request(&grant, effect.clone()))
        .await
        .unwrap();
    f.effect(&proof).await.unwrap();
    f.resources
        .resource_policies
        .update_one(
            doc! {"_id":grant.target.ids.sandbox_id.as_str()},
            doc! {"$set":{"record.organization_evidence.issuer":"https://other.example"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        f.service.authorize(f.request(&grant, effect.clone())).await,
        Err(BridgeInvokeError::TargetMismatch)
    ));
    f.resources
        .resource_policies
        .delete_one(doc! {"_id":grant.target.ids.sandbox_id.as_str()})
        .await
        .unwrap();
    assert!(matches!(
        f.service.authorize(f.request(&grant, effect)).await,
        Err(BridgeInvokeError::TargetMismatch)
    ));
    f.service
        .revoke(
            &f.context,
            Some("fixture-credential"),
            f.revoke(&grant, "admin-revoke"),
        )
        .await
        .unwrap();
    f.close().await;
}
