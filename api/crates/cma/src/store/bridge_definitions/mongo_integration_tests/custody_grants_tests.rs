use super::*;

const INCARNATION: &str = "fixture-deliberate-oauth-sign-in-incarnation";

fn with_grant(mut material: StoredBridgeCredential) -> StoredBridgeCredential {
    let subject = match &material.metadata.created_by {
        layer2_core::resource_access::ResourceActor::Human { subject }
        | layer2_core::resource_access::ResourceActor::Api {
            subject,
            delegation_id: None,
        } => subject.clone(),
        _ => panic!("human fixture"),
    };
    let now = std::time::SystemTime::now();
    material.control_plane = Some(crate::bridge::control_plane::BridgeControlPlaneGrant {
        bridge_id: material.metadata.bridge_id.clone(),
        owner: material.metadata.owner.clone(),
        connector: material.metadata.connector.clone(),
        subject,
        incarnation: INCARNATION.into(),
        issued_at: layer2_core::rfc3339_at(now),
        expires_at: layer2_core::rfc3339_at(now + crate::bridge::control_plane::GRANT_LIFETIME),
    });
    material
}

struct OrgManagers;
#[async_trait::async_trait]
impl OrganizationAuthority for OrgManagers {
    async fn organization_context(
        &self,
        invoking: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut context = caller();
        context.caller = invoking.clone();
        context.credential.subject = invoking.subject.clone();
        context.actor = ResourceActor::Human {
            subject: invoking.subject.clone(),
        };
        let now = std::time::SystemTime::now();
        context
            .memberships
            .push(layer2_core::resource_access::OrganizationMembership::new(
                context.credential.clone(),
                organization.clone(),
                layer2_core::resource_access::OrganizationRole::Admin,
                now,
                now + Duration::from_secs(30),
            )?);
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn renewable_grant_reconnect_records_new_org_admin_without_rewriting_original_key_importer() {
    let (client, database, definitions) = fixture().await;
    let svc = BridgeService::new(definitions, Arc::new(OrgManagers), Arc::new(Authority));
    let id = BridgeService::creation_id(&caller(), "create-org-grant").unwrap();
    svc.mutate(
        &caller(),
        Some("fixture-human"),
        change(
            &id,
            0,
            "create-org-grant",
            BridgeChange::Create {
                owner: ResourceOwner::Organization {
                    subject: SubjectId::new("org-fixture"),
                },
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    let first = svc
        .management_lease(&caller(), Some("fixture-human"), &id, 1)
        .await
        .unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let reference = material(&first).metadata.connector.custody;
    let original = store
        .admit(&first, with_grant(material(&first)))
        .await
        .unwrap();
    let mut second = caller();
    second.caller.subject = SubjectId::new("new-org-admin");
    second.credential.subject = second.caller.subject.clone();
    second.actor = ResourceActor::Human {
        subject: second.caller.subject.clone(),
    };
    let management = svc
        .management_lease(&second, Some("fixture-human"), &id, 1)
        .await
        .unwrap();
    let replacement = with_grant(material(&management));
    let expected = replacement.control_plane.clone();
    assert_eq!(
        store.admit(&management, replacement).await.unwrap(),
        original
    );
    let current = store.get(&reference).await.unwrap().unwrap();
    assert_eq!(current.metadata.created_by, caller().actor);
    assert_eq!(current.control_plane, expected);
    assert_eq!(
        current.control_plane.unwrap().subject,
        second.caller.subject
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn renewable_grant_attaches_only_deliberately_and_reopens_with_original_bounded_expiry() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = BridgeService::creation_id(&caller(), "create-renewable-grant").unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create-renewable-grant",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let reference = material(&management).metadata.connector.custody;
    let original = store
        .admit(&management, material(&management))
        .await
        .unwrap();
    assert!(store
        .get(&reference)
        .await
        .unwrap()
        .unwrap()
        .control_plane
        .is_none());
    let input = with_grant(material(&management));
    let expected = input.control_plane.clone().unwrap();
    assert_eq!(store.admit(&management, input).await.unwrap(), original);
    let raw = client
        .database(&database)
        .collection::<Document>("bridge_credentials")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{raw:?}").contains(SECRET));
    assert!(!format!("{raw:?}").contains(INCARNATION));
    assert!(raw.contains_key("control_plane"));
    let reopened =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    assert_eq!(
        reopened
            .get(&reference)
            .await
            .unwrap()
            .unwrap()
            .control_plane,
        Some(expected.clone())
    );
    let mut retry = with_grant(material(&management));
    retry.control_plane.as_mut().unwrap().issued_at = "2090-01-01T00:00:00Z".into();
    retry.control_plane.as_mut().unwrap().expires_at = "2090-01-31T00:00:00Z".into();
    reopened.admit(&management, retry).await.unwrap();
    assert_eq!(
        reopened
            .get(&reference)
            .await
            .unwrap()
            .unwrap()
            .control_plane,
        Some(expected)
    );
    // Older requests cannot remove or lengthen a deliberate grant.
    reopened
        .admit(&management, material(&management))
        .await
        .unwrap();
    assert!(reopened
        .get(&reference)
        .await
        .unwrap()
        .unwrap()
        .control_plane
        .is_some());
    reopened
        .revoke(&management, &reference, 1, &"b".repeat(64))
        .await
        .unwrap();
    assert!(reopened
        .get(&reference)
        .await
        .unwrap()
        .unwrap()
        .metadata
        .revoked_at
        .is_some());
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_credentials")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn renewable_grant_ciphertext_and_owner_cannot_be_substituted_or_admitted_under_stale_definition(
) {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = BridgeService::creation_id(&caller(), "create-renewable-boundary").unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create-renewable-boundary",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let reference = material(&management).metadata.connector.custody;
    let mut wrong = with_grant(material(&management));
    wrong.control_plane.as_mut().unwrap().subject = SubjectId::new("unrelated-person");
    assert!(matches!(
        store.admit(&management, wrong).await,
        Err(BridgeError::Corrupt)
    ));
    assert!(store.get(&reference).await.unwrap().is_none());
    store
        .admit(&management, with_grant(material(&management)))
        .await
        .unwrap();
    let collection = client
        .database(&database)
        .collection::<Document>("bridge_credentials");
    let raw = collection.find_one(doc! {}).await.unwrap().unwrap();
    collection
        .update_one(
            doc! {"_id":reference.as_str()},
            doc! {"$set":{"control_plane":raw.get("secret").unwrap()}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.get(&reference).await,
        Err(BridgeError::Corrupt)
    ));
    collection
        .replace_one(doc! {"_id":reference.as_str()}, raw)
        .await
        .unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            1,
            "stop-renewable",
            BridgeChange::Emergency {
                controls: BridgeEmergencyControls {
                    ingress_stopped: true,
                    ..Default::default()
                },
            },
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        store
            .admit(&management, with_grant(material(&management)))
            .await,
        Err(BridgeError::Conflict)
    ));
    client.database(&database).drop().await.unwrap();
}
