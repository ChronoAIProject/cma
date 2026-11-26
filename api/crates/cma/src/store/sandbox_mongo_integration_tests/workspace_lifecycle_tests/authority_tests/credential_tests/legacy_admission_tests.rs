use super::*;

pub(super) async fn independent_custody(f: &Fixture) -> Arc<MongoSandboxCredentialStore> {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    Arc::new(
        MongoSandboxCredentialStore::open(&client, &f.name, "work-contract", &f.key)
            .await
            .unwrap(),
    )
}

pub(super) fn retained(f: &Fixture) -> CredentialMaterial {
    CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: f.claim.id.clone(),
        owner: f.claim.owner.clone(),
        authority: SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: "retained-legacy-key".into(),
            provider_state_version: 1,
            issued_at: at(-10),
            expires_at: at(3600),
            grant_fingerprint: "a".repeat(64),
        },
        value: SecretValue::new(format!("fixture_{}", uuid::Uuid::new_v4().simple())),
    }
}

async fn assert_empty(f: &Fixture) {
    for name in ["sandbox_credentials", "sandbox_credential_issuance"] {
        assert_eq!(
            f.client
                .database(&f.name)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_legacy_controllers_and_restart_never_dispatch_a_fresh_mint() {
    let mut f = Fixture::new_mode("legacy_refusal", false).await;
    assert!(f.claim.work.authority_protocol.is_none());
    let other = independent_custody(&f).await;
    let second = ManagedSandboxCredentials::new(f.issuer.clone(), Arc::new(Tokens), other);
    let a = f.controller.clone();
    let b = second.clone();
    let claim = f.claim.clone();
    let mut issuing = tokio::spawn(async move {
        tokio::join!(
            a.issue(&claim, 1, SystemTime::now()),
            b.issue(&claim, 1, SystemTime::now())
        )
    });
    let results = loop {
        tokio::select! {
            result = &mut issuing => break result.unwrap(),
            request = f.peer.next() => {
                match request.uri.path() {
                    "/api/v1/user-services" => request.json(json!([{"id":"fixture-service-id","slug":"fixture-service","is_active":true}])),
                    "/api/v1/api-keys/scope-plan" => {
                        let body: Value = serde_json::from_slice(&request.body).unwrap();
                        request.json(crate::auth::keys::scope_plan_fixture(f.claim.owner.as_str(), &body, &["fixture-node"]));
                    }
                    "/api/v1/api-keys" => { accepted(request); }
                    _ => request.respond(204, ""),
                }
            }
        }
    };
    assert_eq!(
        f.peer.count_route("POST", "/api/v1/api-keys"),
        0,
        "unprotected mint must never dispatch"
    );
    for result in [results.0, results.1] {
        assert!(matches!(result, Err(CredentialError::Unavailable(reason))
            if reason == "managed credential issuance requires protected work authority"));
    }
    let restarted = ManagedSandboxCredentials::new(
        f.issuer.clone(),
        Arc::new(Tokens),
        independent_custody(&f).await,
    );
    assert!(matches!(
        restarted.issue(&f.claim, 1, SystemTime::now()).await,
        Err(CredentialError::Unavailable(_))
    ));
    assert_eq!(
        f.peer.count(),
        0,
        "refusal precedes provider authority acquisition"
    );
    assert_empty(&f).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_legacy_custody_remains_readable_and_revocable_after_admission_closes() {
    let mut f = Fixture::new_mode("legacy_retained", false).await;
    let material = retained(&f);
    f.custody.seed_retained_for_test(&material).await;
    let raw = f
        .client
        .database(&f.name)
        .collection::<Document>("sandbox_credentials")
        .find_one(doc! {"sandbox_id":f.claim.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!raw.contains_key("source"));
    let second = ManagedSandboxCredentials::new(
        f.issuer.clone(),
        Arc::new(Tokens),
        independent_custody(&f).await,
    );
    let recovered = second.issue(&f.claim, 1, SystemTime::now()).await.unwrap();
    assert_eq!(recovered.authority, material.authority);
    assert_eq!(recovered.value.expose(), material.value.expose());
    assert!(matches!(
        second.issue(&f.claim, 2, SystemTime::now()).await,
        Err(CredentialError::Unavailable(_))
    ));
    for source in [false, true] {
        let mut wrong = f.claim.clone();
        if source {
            wrong.credential.source = layer2_core::sandbox::SandboxCredentialSource::Provided {
                key_sha256: "a".repeat(64),
            };
        } else {
            wrong.owner = SubjectId::new("foreign-owner");
        }
        assert!(matches!(
            second.issue(&wrong, 1, SystemTime::now()).await,
            Err(CredentialError::Security(_))
        ));
    }
    assert_eq!(second.custodied(&f.claim.id).await.unwrap().len(), 1);
    let claim = f.claim.clone();
    let controller = second.clone();
    let revoking =
        tokio::spawn(async move { controller.revoke(&claim, "retained-legacy-key").await });
    let request = f.peer.next().await;
    assert_eq!(request.method, axum::http::Method::DELETE);
    assert_eq!(request.uri.path(), "/api/v1/api-keys/retained-legacy-key");
    request.respond(204, "");
    revoking.await.unwrap().unwrap();
    second.forget(&f.claim.id, 1).await.unwrap();
    assert_empty(&f).await;
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn managed_raw_insert_cannot_follow_retirement_final_scan_and_committed_purge() {
    let f = Fixture::new_mode("legacy_purge", false).await;
    let delayed = independent_custody(&f).await;
    let material = retained(&f);
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let late = tokio::spawn(async move {
        ready.send(()).unwrap();
        released.await.unwrap();
        delayed.insert(material).await
    });
    waiting.await.unwrap();
    let (worker, scans) = super::retirement_fixture::retire(&f).await;
    assert!(worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    let purging = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(purging.work.step, SandboxWorkStep::Purge);
    let before = scans.scans.load(std::sync::atomic::Ordering::SeqCst);
    assert!(worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    assert_eq!(
        scans.scans.load(std::sync::atomic::Ordering::SeqCst),
        before + 1,
        "final empty custody scan ran"
    );
    assert!(
        peer(&f.name)
            .await
            .get_sandbox(&f.claim.id)
            .await
            .unwrap()
            .is_none(),
        "parent deletion committed"
    );
    release.send(()).unwrap();
    assert!(
        late.await.unwrap().is_err(),
        "late managed custody must be refused after purge"
    );
    assert_empty(&f).await;
    assert!(
        f.custody.insert(retained(&f)).await.is_err(),
        "absent parent cannot admit managed custody"
    );
    assert_eq!(f.peer.count(), 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_legacy_claim_and_retired_parent_cannot_admit_raw_managed_custody() {
    let f = Fixture::new_mode("legacy_stale_retired", false).await;
    assert!(f.custody.insert(retained(&f)).await.is_err());
    let captured = f.claim.clone();
    let _ = super::retirement_fixture::retire(&f).await;
    assert!(f.custody.insert(retained(&f)).await.is_err());
    assert!(matches!(
        f.controller.issue(&captured, 1, SystemTime::now()).await,
        Err(CredentialError::Unavailable(_))
    ));
    assert_empty(&f).await;
    assert_eq!(f.peer.count(), 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn protected_late_result_refuses_after_retirement_and_takeover_without_losing_intent() {
    use layer2_core::profiles::store::InMemoryProfileStore;
    use layer2_core::sandbox::{FixedSandboxCredentialPrerequisite, SandboxPolicy, SandboxService};
    for takeover in [false, true] {
        let mut f = Fixture::new("protected_late_result").await;
        let original = f.issue(1);
        let mint = f.prepare().await;
        let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
        if takeover {
            f.takeover().await;
        } else {
            let service = SandboxService::new(
                Arc::new(f.store.clone()),
                Arc::new(InMemoryProfileStore::default()),
                SandboxPolicy::default(),
                Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            );
            let owner = Caller::new(
                f.claim.owner.clone(),
                Authorization::new([Capability::DeleteOwnSandbox]),
            );
            assert_eq!(
                service.delete(&owner, &f.claim.id).await.unwrap().status,
                SandboxStatus::Retiring
            );
        }
        accepted(mint);
        let result = original.await.unwrap();
        assert!(
            !matches!(result, Ok(Ok(_))),
            "stale accepted result must not publish custody"
        );
        assert_eq!(f.custody.issuance(&f.claim).await.unwrap(), Some(intent));
        assert!(f.custody.get(&f.claim.id, 1).await.unwrap().is_none());
        assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
        f.cleanup().await;
    }
}
