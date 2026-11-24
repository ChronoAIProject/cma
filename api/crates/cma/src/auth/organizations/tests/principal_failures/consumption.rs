use super::*;
use crate::auth::identity_flight::{Epoch, IdentityStore, MemoryIdentityStore, TokenHash};
use crate::auth::{
    identity::VerifiedSubject,
    identity_evidence::{EvidenceScope, EvidenceStamp},
    identity_lifetime,
    permissions::PermissionEvidence,
    profile::Profile,
};

struct InvalidateAfterPublication(MemoryIdentityStore);

impl IdentityStore for InvalidateAfterPublication {
    fn key(&self, token: &str) -> TokenHash {
        self.0.key(token)
    }
    fn token_epoch(&self, key: &TokenHash) -> Option<Epoch> {
        self.0.token_epoch(key)
    }
    fn token_current(&self, key: &TokenHash, epoch: &Epoch) -> bool {
        self.0.token_current(key, epoch)
    }
    fn evidence_stamp(
        &self,
        key: &TokenHash,
        scope: EvidenceScope<'_>,
        evidence: &VerifiedSubject,
    ) -> Result<EvidenceStamp, ProviderVerificationError> {
        self.0.evidence_stamp(key, scope, evidence)
    }
    fn finish_verification(
        &self,
        key: &TokenHash,
        epoch: &Epoch,
        stamp: &EvidenceStamp,
    ) -> Option<Epoch> {
        let published = self.0.finish_verification(key, epoch, stamp)?;
        // Invalidate the fixture store between a successful publication and
        // the actual bearer consumer's recheck of that receipt.
        self.0.clear();
        Some(published)
    }
    fn invalidate_token(&self, token: &str) {
        self.0.invalidate_token(token);
    }
    fn clear(&self) {
        self.0.clear();
    }
    fn active_receipts(&self) -> usize {
        self.0.active_receipts()
    }
}

#[tokio::test]
async fn actual_bearer_consumer_retains_epoch_failure_after_provider_success() {
    let mut f = Fixture::new().await;
    Arc::get_mut(&mut f.auth).unwrap().identity_store =
        Arc::new(InvalidateAfterPublication(MemoryIdentityStore::default()));
    let ((status, body), events) =
        capture(context("sha256:consumption-fixture").scope(response(&f))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    fields(
        &events,
        "principal_verification",
        "consumption_epoch_changed",
    );
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn current_receipts_distinguish_expiry_from_invalidation() {
    let auth = AuthState::in_memory(
        AuthMode::Provider {
            base_url: "https://identity.test".into(),
            client_id: "fixture".into(),
            client_secret: "fixture".into(),
            public_url: "https://cma.test".into(),
        },
        None,
    );
    let key = auth.identity_store.key("fixture-credential");
    let epoch = auth.identity_store.token_epoch(&key).unwrap();
    let mut evidence = VerifiedSubject {
        subject: caller().subject,
        permissions: PermissionEvidence::Active(vec![]),
        profile: Profile::default(),
        independent_commands: false,
        api_expires_at: None,
        token_expires_at: None,
        verification: Some(
            identity_lifetime::VerificationLifetime::new(
                key,
                epoch,
                Some(SystemTime::now() + Duration::from_secs(5)),
            )
            .unwrap(),
        ),
    };
    assert!(identity_lifetime::current(&auth, &evidence));
    tokio::time::advance(Duration::from_secs(6)).await;
    assert_eq!(
        identity_lifetime::failure(&auth, &evidence),
        Some(VerificationCause::ConsumptionExpired)
    );
    evidence.verification = None;
    evidence.token_expires_at = Some(SystemTime::UNIX_EPOCH);
    assert_eq!(
        identity_lifetime::failure(&auth, &evidence),
        Some(VerificationCause::ConsumptionExpired)
    );
    evidence.token_expires_at = None;
    let epoch = auth.identity_store.token_epoch(&key).unwrap();
    evidence.verification =
        Some(identity_lifetime::VerificationLifetime::new(key, epoch, None).unwrap());
    auth.identity_store.invalidate_token("fixture-credential");
    assert_eq!(
        identity_lifetime::failure(&auth, &evidence),
        Some(VerificationCause::ConsumptionEpochChanged)
    );
    assert!(!identity_lifetime::current(&auth, &evidence));
}
