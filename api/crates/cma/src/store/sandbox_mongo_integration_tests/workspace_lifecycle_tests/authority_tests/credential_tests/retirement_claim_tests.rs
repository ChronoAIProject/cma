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
        // Simulate an old v1 lease with no indexed schedule, then expire it.
        records.update_one(filter.clone(), doc! {"$set":{"retirement_custody.claim.expires_at":DateTime::from_system_time(SystemTime::now() - std::time::Duration::from_secs(1))}, "$unset":{"retirement_discovery_at":""}}).await.unwrap();
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
    refused.insert("retirement_discovery_at", past);
    refused
        .get_document_mut("retirement_custody")
        .unwrap()
        .insert("retry_at", past);
    records
        .update_one(
            filter.clone(),
            doc! {"$set":{"retirement_custody.retry_at":DateTime::from_millis(1),"retirement_discovery_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let refused_filter = doc! {"_id":refused_sandbox.id.as_str()};
    records.insert_one(refused.clone()).await.unwrap();

    // More than one bounded page of v1 records has no indexed schedule.
    // Their leases, backoff and refusal deadlines are all in the future.
    let soon = DateTime::from_system_time(SystemTime::now() + std::time::Duration::from_secs(600));
    let latest =
        DateTime::from_system_time(SystemTime::now() + std::time::Duration::from_secs(900));
    let mut legacy = Vec::new();
    for i in 0..17 {
        let mut row = original.clone();
        let id = SandboxId::generate();
        row.insert("_id", id.as_str());
        let intent = row.get_document_mut("record").unwrap();
        intent.insert("sandbox_id", id.as_str());
        intent.insert("operation_id", format!("mint_{}", uuid::Uuid::new_v4()));
        let mut frozen = intent.clone();
        frozen.remove("next_check_at");
        let marker = row.get_document_mut("retirement_custody").unwrap();
        marker.insert("issuance", frozen);
        marker
            .get_document_mut("late_provider_authority")
            .unwrap()
            .insert("provider_key_id", format!("legacy-fixture-key-{i}"));
        marker.insert("retry_at", soon);
        if i % 2 == 0 {
            marker.insert(
                "claim",
                doc! {"holder":"legacy-holder", "token":"legacy-token", "expires_at":latest},
            );
        } else {
            row.insert(
                "retirement_discovery_refusal",
                doc! {"reason":"invalid_retirement_identity","observed_at":past,"retry_at":latest},
            );
        }
        assert!(!row.contains_key("retirement_discovery_at"));
        legacy.push(row);
    }
    records.insert_many(legacy.clone()).await.unwrap();
    let first_scan = independent_custody(&f).await;
    assert!(first_scan
        .claim_retirement_revocation("only-legacy-page")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        records
            .count_documents(doc! {"retirement_discovery_at":latest})
            .await
            .unwrap(),
        16
    );
    drop(first_scan);

    let independent = independent_custody(&f).await;
    let claim = independent
        .claim_retirement_revocation("first-owner-scan")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.issuance.sandbox_id, f.claim.id);
    assert_eq!(claim.issuance.owner, f.claim.owner);
    for previous in legacy {
        let mut read = records
            .find_one(doc! {"_id":previous.get_str("_id").unwrap()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read.remove("retirement_discovery_at"), Some(latest.into()));
        // No initial lease or refusal was replaced and no new claim was made.
        assert_eq!(read, previous);
    }

    let mut observed = records
        .find_one(refused_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let refusal = observed.remove("retirement_discovery_refusal").unwrap();
    let scheduled = observed.get_datetime("retirement_discovery_at").unwrap();
    let refusal = refusal.as_document().unwrap();
    assert_eq!(
        refusal.get_str("reason").unwrap(),
        "invalid_retirement_identity"
    );
    assert!(
        refusal.get_datetime("retry_at").unwrap() > refusal.get_datetime("observed_at").unwrap()
    );
    assert_eq!(scheduled, refusal.get_datetime("retry_at").unwrap());
    observed.insert("retirement_discovery_at", past);
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
            doc! {"$set":{"retirement_custody.claim.expires_at":past,"retirement_discovery_at":past}},
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_retirement_effect_admission_retains_custody_across_erasure() {
    use crate::store::account_deletions::MongoAccountDeletionStore;
    use layer2_core::account_deletion::AccountDeletionStore;

    for erasure_first in [true, false] {
        let mut f = Fixture::new_with_runtime("retirement_effect_erasure", true, true).await;
        f.controller = ManagedSandboxCredentials::new(
            f.issuer.clone(),
            Arc::new(ExactTokens),
            f.custody.clone(),
        );
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

        let database = f.client.database(&f.name);
        let deletions = MongoAccountDeletionStore::open(&database).await.unwrap();
        let records = database.collection::<Document>("sandbox_credential_issuance");
        let filter = doc! {"_id":f.claim.id.as_str()};
        let independent = independent_custody(&f).await;
        let claim = independent
            .claim_retirement_revocation("original-dispatch")
            .await
            .unwrap()
            .unwrap();
        let issuer = f.issuer.provider_identity();
        assert!(independent
            .admit_retirement_revocation(&claim, &"b".repeat(64))
            .await
            .is_err());
        if erasure_first {
            deletions
                .request(&claim.issuance.owner, "fixture-erasure-first", &at(0))
                .await
                .unwrap();
            assert!(independent
                .admit_retirement_revocation(&claim, &issuer)
                .await
                .unwrap()
                .is_none());
            let row = records.find_one(filter.clone()).await.unwrap().unwrap();
            assert!(!row
                .get_document("retirement_custody")
                .unwrap()
                .contains_key("revocation_dispatch"));
        } else {
            let admission = independent
                .admit_retirement_revocation(&claim, &issuer)
                .await
                .unwrap()
                .unwrap();
            assert!(independent
                .retirement_revocation_admission_current(&admission)
                .await
                .unwrap());
            assert!(independent
                .admit_retirement_revocation(&claim, &issuer)
                .await
                .unwrap()
                .is_none());
            let original = records.find_one(filter.clone()).await.unwrap().unwrap();
            let dispatch = original
                .get_document("retirement_custody")
                .unwrap()
                .get_document("revocation_dispatch")
                .unwrap();
            assert_eq!(dispatch.get_str("last_claim_token").unwrap(), claim.token);
            assert_eq!(dispatch.get_i64("attempts").unwrap(), 1);
            assert_eq!(
                dispatch
                    .get_document("identity")
                    .unwrap()
                    .get_str("provider_identity")
                    .unwrap(),
                issuer
            );

            // Lost acknowledgement followed by lease expiry permits only a
            // successor claim for the same exact original effect.
            let past = DateTime::from_millis(0);
            records.update_one(filter.clone(), doc! {"$set":{"retirement_custody.claim.expires_at":past,"retirement_discovery_at":past}}).await.unwrap();
            drop(independent);
            let restarted = independent_custody(&f).await;
            let successor = restarted
                .claim_retirement_revocation("successor-dispatch")
                .await
                .unwrap()
                .unwrap();
            assert!(!restarted
                .retirement_revocation_admission_current(&admission)
                .await
                .unwrap());
            assert!(restarted
                .admit_retirement_revocation(&claim, &issuer)
                .await
                .unwrap()
                .is_none());
            let next = restarted
                .admit_retirement_revocation(&successor, &issuer)
                .await
                .unwrap()
                .unwrap();
            let read = records.find_one(filter.clone()).await.unwrap().unwrap();
            let second = read
                .get_document("retirement_custody")
                .unwrap()
                .get_document("revocation_dispatch")
                .unwrap();
            assert_eq!(
                second.get_document("identity").unwrap(),
                dispatch.get_document("identity").unwrap()
            );
            assert_eq!(
                second.get_datetime("first_admitted_at").unwrap(),
                dispatch.get_datetime("first_admitted_at").unwrap()
            );
            assert_eq!(second.get_i64("attempts").unwrap(), 2);
            assert_eq!(second.get_str("last_claim_token").unwrap(), successor.token);

            // Admission won its transaction. The later erasure cannot erase
            // the retained obligation or admit a new attempt under its consent.
            deletions
                .request(
                    &claim.issuance.owner,
                    "fixture-erasure-after-dispatch",
                    &at(0),
                )
                .await
                .unwrap();
            assert!(deletions.erasing(&claim.issuance.owner).await.unwrap());
            assert!(restarted
                .retirement_revocation_admission_current(&next)
                .await
                .unwrap());
            records.update_one(filter.clone(), doc! {"$set":{"retirement_custody.claim.expires_at":past,"retirement_discovery_at":past}}).await.unwrap();
            let after_erasure = restarted
                .claim_retirement_revocation("after-erasure")
                .await
                .unwrap()
                .unwrap();
            assert!(restarted
                .admit_retirement_revocation(&after_erasure, &issuer)
                .await
                .unwrap()
                .is_none());
            let after = records.find_one(filter).await.unwrap().unwrap();
            assert_eq!(
                after
                    .get_document("retirement_custody")
                    .unwrap()
                    .get_document("revocation_dispatch")
                    .unwrap(),
                second
            );
        }
        let custody = independent_custody(&f).await;
        assert!(custody.requires_no_resend(&f.claim).await.unwrap());
        assert_eq!(
            custody.issuance(&f.claim).await.unwrap().unwrap().state,
            IssuanceState::Unknown
        );
        assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
        assert_eq!(
            f.peer
                .count_route("DELETE", "/api/v1/api-keys/fixture-provider-key"),
            0
        );
        f.cleanup().await;
    }
}

struct RevocationTokens {
    expected: RetirementOwnerAuthority,
    available: bool,
}

#[async_trait::async_trait]
impl crate::sandbox::retirement_revocation::RetirementOwnerTokenSource for RevocationTokens {
    async fn exact_token(&self, authority: &RetirementOwnerAuthority) -> Option<SecretValue> {
        assert_eq!(&self.expected, authority);
        self.available
            .then(|| SecretValue::new("synthetic-retained-owner-token".into()))
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_retirement_effect_worker_requires_retained_authority_and_exact_200() {
    use crate::sandbox::retirement_revocation::RetirementRevocationWorker;
    use crate::store::account_deletions::MongoAccountDeletionStore;
    use layer2_core::account_deletion::AccountDeletionStore;

    for case in [
        "missing_authority",
        "erasure_first",
        "wrong_issuer",
        "ambiguous",
        "stale_claim",
        "confirmed",
    ] {
        let mut f = Fixture::new_with_runtime("retirement_revocation_worker", true, true).await;
        f.controller = ManagedSandboxCredentials::new(
            f.issuer.clone(),
            Arc::new(ExactTokens),
            f.custody.clone(),
        );
        let issuing = f.issue(1);
        let mint = f.prepare().await;
        let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
        let (_worker, _) = retirement_fixture::begin(&f).await;
        let (worker, _) = retirement_fixture::native_custody_worker(&f, true, true);
        assert!(worker
            .run_one(&f.claim.id, SystemTime::now())
            .await
            .unwrap());
        let secret = accepted(mint);
        assert!(matches!(
            issuing.await.unwrap().unwrap(),
            Err(CredentialError::Uncertain)
        ));
        let database = f.client.database(&f.name);
        let records = database.collection::<Document>("sandbox_credential_issuance");
        let filter = doc! {"_id":f.claim.id.as_str()};
        let before = records.find_one(filter.clone()).await.unwrap().unwrap();
        let provider_authority: SandboxCredentialAuthority = mongodb::bson::deserialize_from_bson(
            before
                .get_document("retirement_custody")
                .unwrap()
                .get("late_provider_authority")
                .unwrap()
                .clone(),
        )
        .unwrap();
        let deletions = MongoAccountDeletionStore::open(&database).await.unwrap();
        if case == "erasure_first" {
            deletions
                .request(&f.claim.owner, "owner-erasure", &at(0))
                .await
                .unwrap();
        }
        let issuer = if case == "wrong_issuer" {
            Arc::new(KeyIssuer::new(
                reqwest::Client::new(),
                &f.peer.origin,
                "other-deployment",
            ))
        } else {
            f.issuer.clone()
        };
        let tokens = Arc::new(RevocationTokens {
            expected: selector(&f.claim.owner),
            available: case != "missing_authority",
        });
        let independent = independent_custody(&f).await;
        let revoker = RetirementRevocationWorker::new(
            independent.clone(),
            issuer,
            tokens.clone(),
            "first-revoker".into(),
        );
        let dispatches = matches!(case, "ambiguous" | "stale_claim" | "confirmed");
        if dispatches {
            let task = tokio::spawn(async move { revoker.run_one().await });
            let request = f.peer.next().await;
            assert_eq!(request.method.as_str(), "DELETE");
            assert_eq!(request.uri.path(), "/api/v1/api-keys/fixture-provider-key");
            let during = records.find_one(filter.clone()).await.unwrap().unwrap();
            assert!(during
                .get_document("retirement_custody")
                .unwrap()
                .contains_key("revocation_dispatch"));
            if case == "stale_claim" {
                records.update_one(filter.clone(), doc! {"$set":{"retirement_custody.claim.expires_at":DateTime::from_millis(0),"retirement_discovery_at":DateTime::from_millis(0)}}).await.unwrap();
                assert!(independent
                    .claim_retirement_revocation("takeover")
                    .await
                    .unwrap()
                    .is_some());
            }
            if case == "confirmed" {
                // Erasure admitted after the durable effect must retain its
                // exact disposition through an in-flight success response.
                deletions
                    .request(&f.claim.owner, "later-erasure", &at(0))
                    .await
                    .unwrap();
            }
            request.respond(
                if case == "ambiguous" { 202 } else { 200 },
                "fixture response",
            );
            assert!(task.await.unwrap().unwrap());
        } else {
            assert!(revoker.run_one().await.unwrap());
        }
        let observed = records.find_one(filter.clone()).await.unwrap().unwrap();
        let marker = observed.get_document("retirement_custody").unwrap();
        assert_eq!(marker.contains_key("revocation_dispatch"), dispatches);
        assert_eq!(
            marker.contains_key("revocation_observation"),
            case == "confirmed"
        );
        assert_eq!(
            marker.get_str("state").unwrap(),
            if case == "confirmed" {
                "key_revoked"
            } else {
                "pending"
            }
        );
        if case == "confirmed" {
            let receipt = marker.get_document("revocation_observation").unwrap();
            assert_eq!(receipt.get_i64("http_status").unwrap(), 200);
            assert_eq!(
                receipt
                    .get_document("identity")
                    .unwrap()
                    .get_str("provider_key_id")
                    .unwrap(),
                "fixture-provider-key"
            );
            let mut duplicate = retained(&f);
            duplicate.authority = provider_authority;
            duplicate.value = SecretValue::new(secret);
            assert!(f
                .custody
                .retain_late_retirement_result(&f.claim, &intent, &duplicate)
                .await
                .unwrap());
            assert_eq!(
                records.find_one(filter.clone()).await.unwrap().unwrap(),
                observed
            );
            assert!(independent
                .claim_retirement_revocation("completed")
                .await
                .unwrap()
                .is_none());
        }
        if case == "ambiguous" {
            // After a lost or unqualified reply, a restart that gets 404
            // cannot establish original provider finality.
            records.update_one(filter.clone(), doc! {"$set":{"retirement_custody.retry_at":DateTime::from_millis(0),"retirement_discovery_at":DateTime::from_millis(0)}}).await.unwrap();
            let restarted = RetirementRevocationWorker::new(
                independent_custody(&f).await,
                f.issuer.clone(),
                tokens,
                "restart-revoker".into(),
            );
            let task = tokio::spawn(async move { restarted.run_one().await });
            f.peer.next().await.respond(404, "fixture unknown result");
            assert!(task.await.unwrap().unwrap());
            let row = records.find_one(filter.clone()).await.unwrap().unwrap();
            assert!(!row
                .get_document("retirement_custody")
                .unwrap()
                .contains_key("revocation_observation"));
        }
        assert_eq!(
            independent.issuance(&f.claim).await.unwrap().unwrap().state,
            IssuanceState::Unknown
        );
        assert!(independent.requires_no_resend(&f.claim).await.unwrap());
        assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
        assert_eq!(
            f.peer
                .count_route("DELETE", "/api/v1/api-keys/fixture-provider-key"),
            if case == "ambiguous" {
                2
            } else {
                usize::from(dispatches)
            }
        );
        f.cleanup().await;
    }
}
