use super::legacy_admission_tests::{independent_custody, retained};
use super::*;
use crate::sandbox::{IssuanceState, RetirementOwnerAuthority};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use mongodb::bson::DateTime;

struct ExactTokens;
fn selector(owner: &SubjectId) -> RetirementOwnerAuthority {
    RetirementOwnerAuthority::Personal {
        owner: owner.clone(),
        authorization: AgentAuthorizationRevision {
            authority_id: format!("personal:{}", "a".repeat(64)),
            policy_revision: 1,
            grant_revision: None,
            replay_principal: None,
        },
    }
}
#[async_trait::async_trait]
impl OwnerTokenSource for ExactTokens {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(Some(SecretValue::new("synthetic-owner-token".into())))
    }
    async fn token_for_issuance(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<(SecretValue, Option<RetirementOwnerAuthority>)>, String> {
        Ok(Some((
            self.token_for(&sandbox.owner).await?.unwrap(),
            Some(selector(&sandbox.owner)),
        )))
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_retirement_claim_retains_exact_owner_and_survives_takeover() {
    for marker_first in [true, false] {
        let mut f = Fixture::new_with_runtime("independent_retirement_claim", true, true).await;
        f.controller = ManagedSandboxCredentials::new(
            f.issuer.clone(),
            Arc::new(ExactTokens),
            f.custody.clone(),
        );
        let issuing = f.issue(1);
        let mint = f.prepare().await;
        let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
        let records = f
            .client
            .database(&f.name)
            .collection::<Document>("sandbox_credential_issuance");
        let filter = doc! {"_id":f.claim.id.as_str()};
        let original = records.find_one(filter.clone()).await.unwrap().unwrap();
        let captured = mongodb::bson::serialize_to_bson(&selector(&f.claim.owner)).unwrap();
        assert_eq!(original.get("issuance_owner_authority"), Some(&captured));
        let (_worker, _) = retirement_fixture::begin(&f).await;
        let (worker, _) = retirement_fixture::native_custody_worker(&f, true, true);
        if marker_first {
            assert!(worker
                .run_one(&f.claim.id, SystemTime::now())
                .await
                .unwrap());
            assert!(f
                .custody
                .claim_retirement_revocation("before-result")
                .await
                .unwrap()
                .is_none());
        }
        let secret = accepted(mint);
        assert!(matches!(
            issuing.await.unwrap().unwrap(),
            Err(CredentialError::Uncertain)
        ));
        if !marker_first {
            assert!(worker
                .run_one(&f.claim.id, SystemTime::now())
                .await
                .unwrap());
        }
        let row = records.find_one(filter.clone()).await.unwrap().unwrap();
        let marker = row.get_document("retirement_custody").unwrap();
        assert_eq!(marker.get("owner_authority"), Some(&captured));
        let independent = independent_custody(&f).await;
        if !marker_first {
            assert!(independent
                .claim_retirement_revocation("before-handoff")
                .await
                .unwrap()
                .is_none());
            assert!(!marker.contains_key("late_provider_authority"));
            f.cleanup().await;
            continue;
        }
        let (a, b) = tokio::join!(
            f.custody.claim_retirement_revocation("first"),
            independent.claim_retirement_revocation("second")
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_ne!(a.is_some(), b.is_some());
        let original_claim = a.or(b).unwrap();
        assert_eq!(original_claim.owner_authority, selector(&f.claim.owner));
        assert_eq!(original_claim.issuance.operation_id, intent.operation_id);
        assert_eq!(
            original_claim.provider_authority.provider_key_id,
            "fixture-provider-key"
        );
        let mut duplicate = retained(&f);
        duplicate.authority = original_claim.provider_authority.clone();
        duplicate.value = SecretValue::new(secret);
        let claimed = records.find_one(filter.clone()).await.unwrap().unwrap();
        assert!(f
            .custody
            .retain_late_retirement_result(&f.claim, &intent, &duplicate)
            .await
            .unwrap());
        assert_eq!(
            records.find_one(filter.clone()).await.unwrap().unwrap(),
            claimed
        );
        duplicate.authority.provider_key_id = "other-provider-key".into();
        assert!(f
            .custody
            .retain_late_retirement_result(&f.claim, &intent, &duplicate)
            .await
            .is_err());
        duplicate.authority = original_claim.provider_authority.clone();

        // Simulate the first process losing the claim acknowledgement and
        // restarting. A separate client sees its durable lease until expiry.
        let restarted = independent_custody(&f).await;
        assert!(restarted
            .claim_retirement_revocation("restart-before-expiry")
            .await
            .unwrap()
            .is_none());
        records.update_one(filter.clone(), doc! {"$set":{"retirement_custody.claim.expires_at":DateTime::from_system_time(SystemTime::now() - std::time::Duration::from_secs(1))}}).await.unwrap();
        let successor = restarted
            .claim_retirement_revocation("successor")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(successor.token, original_claim.token);
        assert_eq!(
            successor.provider_authority,
            original_claim.provider_authority
        );
        assert!(!f
            .custody
            .retirement_revocation_claim_current(&original_claim)
            .await
            .unwrap());
        assert!(!f
            .custody
            .defer_retirement_revocation(&original_claim)
            .await
            .unwrap());
        assert!(restarted
            .retirement_revocation_claim_current(&successor)
            .await
            .unwrap());
        assert!(restarted
            .defer_retirement_revocation(&successor)
            .await
            .unwrap());
        let deferred = records.find_one(filter.clone()).await.unwrap().unwrap();
        assert!(f
            .custody
            .retain_late_retirement_result(&f.claim, &intent, &duplicate)
            .await
            .unwrap());
        assert_eq!(records.find_one(filter).await.unwrap().unwrap(), deferred);
        assert!(restarted
            .claim_retirement_revocation("before-backoff")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            deferred
                .get_document("record")
                .unwrap()
                .get_str("state")
                .unwrap(),
            "unknown"
        );
        assert!(f.custody.list(&f.claim.id).await.unwrap().is_empty());
        assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_retirement_claim_skips_refused_owner_across_restart() {
    let mut f = Fixture::new_with_runtime("retirement_discovery_refusal", true, true).await;
    f.controller =
        ManagedSandboxCredentials::new(f.issuer.clone(), Arc::new(ExactTokens), f.custody.clone());
    let issuing = f.issue(1);
    let mint = f.prepare().await;
    let (_worker, _) = retirement_fixture::begin(&f).await;
    let (worker, _) = retirement_fixture::native_custody_worker(&f, true, true);
    assert!(worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    accepted(mint);
    assert!(matches!(
        issuing.await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    let records = f
        .client
        .database(&f.name)
        .collection::<Document>("sandbox_credential_issuance");
    let filter = doc! {"_id":f.claim.id.as_str()};
    let original = records.find_one(filter.clone()).await.unwrap().unwrap();

    // A separate owner's unresolved issuance precedes the valid candidate.
    // Its ordinary issuance record is readable, but the copied retirement
    // marker has a conflicting frozen owner and Sandbox identity.
    let mut refused_sandbox = f.claim.clone();
    refused_sandbox.id = SandboxId::generate();
    refused_sandbox.owner = SubjectId::new("refused-fixture-owner");
    assert_ne!(refused_sandbox.owner, f.claim.owner);
    let mut refused = original.clone();
    refused.insert("_id", refused_sandbox.id.as_str());
    let intent = refused.get_document_mut("record").unwrap();
    intent.insert("sandbox_id", refused_sandbox.id.as_str());
    intent.insert("owner", refused_sandbox.owner.as_str());
    let past = DateTime::from_millis(0);
    refused
        .get_document_mut("retirement_custody")
        .unwrap()
        .insert("retry_at", past);
    records
        .update_one(
            filter.clone(),
            doc! {"$set":{"retirement_custody.retry_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let refused_filter = doc! {"_id":refused_sandbox.id.as_str()};
    records.insert_one(refused.clone()).await.unwrap();

    let independent = independent_custody(&f).await;
    let claim = independent
        .claim_retirement_revocation("first-owner-scan")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.issuance.sandbox_id, f.claim.id);
    assert_eq!(claim.issuance.owner, f.claim.owner);
    let mut observed = records
        .find_one(refused_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let refusal = observed.remove("retirement_discovery_refusal").unwrap();
    let refusal = refusal.as_document().unwrap();
    assert_eq!(
        refusal.get_str("reason").unwrap(),
        "invalid_retirement_identity"
    );
    assert!(
        refusal.get_datetime("retry_at").unwrap() > refusal.get_datetime("observed_at").unwrap()
    );
    assert_eq!(observed, refused);
    assert!(independent
        .requires_no_resend(&refused_sandbox)
        .await
        .unwrap());
    let unknown = independent
        .issuance(&refused_sandbox)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unknown.state, IssuanceState::Unknown);
    assert_eq!(unknown.owner, refused_sandbox.owner);

    drop(independent);
    let restarted = independent_custody(&f).await;
    assert!(restarted
        .claim_retirement_revocation("before-expiry")
        .await
        .unwrap()
        .is_none());
    records
        .update_one(
            filter,
            doc! {"$set":{"retirement_custody.claim.expires_at":past}},
        )
        .await
        .unwrap();
    let successor = restarted
        .claim_retirement_revocation("after-restart")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.issuance.sandbox_id, f.claim.id);
    assert_ne!(successor.token, claim.token);
    let after = records.find_one(refused_filter).await.unwrap().unwrap();
    assert_eq!(
        after.get_document("retirement_discovery_refusal").unwrap(),
        refusal
    );
    assert!(restarted
        .requires_no_resend(&refused_sandbox)
        .await
        .unwrap());
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
    f.cleanup().await;
}
