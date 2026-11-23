use std::sync::Arc;

use super::*;
use crate::{Authorization, Capability};

fn terms(version: &str) -> LegalTerms {
    LegalTerms::new(
        version,
        "https://legal.example.test/terms",
        "https://legal.example.test/privacy",
    )
    .unwrap()
}

fn owner(subject: &str) -> (Caller, OwnerSessionAuthority) {
    (
        Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        OwnerSessionAuthority::verified_personal(SubjectId::new(subject)),
    )
}

fn service(version: Option<&str>, store: Arc<MemoryLegalConsentStore>) -> LegalConsentService {
    LegalConsentService::new(version.map(terms), store)
}

#[test]
fn versions_use_a_closed_bounded_alphabet() {
    for version in [
        "1",
        "2026-11-30",
        "v1.2_rc-3",
        &"a".repeat(MAX_VERSION_LENGTH),
    ] {
        assert!(valid_version(version), "{version}");
    }
    for version in [
        "",
        "two words",
        "v1/2",
        "ünïcode",
        "a$b",
        &"a".repeat(MAX_VERSION_LENGTH + 1),
    ] {
        assert!(!valid_version(version), "{version}");
    }
}

#[test]
fn document_urls_must_be_absolute_https_without_credentials() {
    for (terms_url, privacy_url) in [
        (
            "http://legal.example.test/terms",
            "https://legal.example.test/p",
        ),
        ("https://legal.example.test/t", "/privacy"),
        ("https://", "https://legal.example.test/p"),
        (
            "https://user@legal.example.test/t",
            "https://legal.example.test/p",
        ),
        (
            "https://legal.example.test/t t",
            "https://legal.example.test/p",
        ),
        ("https://legal.example.test/t", "javascript:alert(1)"),
    ] {
        assert!(LegalTerms::new("1", terms_url, privacy_url).is_err());
    }
    let long = format!("https://legal.example.test/{}", "a".repeat(MAX_URL_LENGTH));
    assert!(LegalTerms::new("1", &long, "https://legal.example.test/p").is_err());
    let accepted = LegalTerms::new(
        "1",
        "https://legal.example.test/terms?lang=en#top",
        "https://legal.example.test/privacy",
    )
    .unwrap();
    assert_eq!(accepted.version(), "1");
    assert_eq!(
        accepted.terms_url(),
        "https://legal.example.test/terms?lang=en#top"
    );
    assert_eq!(accepted.privacy_url(), "https://legal.example.test/privacy");
}

#[tokio::test]
async fn unconfigured_deployments_publish_nothing() {
    let service = service(None, Arc::default());
    let (caller, authority) = owner("owner");
    assert!(service.terms().is_none());
    assert_eq!(
        service.status(&caller, Some(&authority)).await,
        Err(LegalConsentError::NotConfigured)
    );
    assert_eq!(
        service.accept(&caller, Some(&authority), "1").await,
        Err(LegalConsentError::NotConfigured)
    );
}

#[tokio::test]
async fn only_the_verified_owner_session_reads_or_records_consent() {
    let store = Arc::new(MemoryLegalConsentStore::default());
    let service = service(Some("1"), store.clone());
    let (caller, _) = owner("owner");
    let (_, other) = owner("someone-else");
    for authority in [None, Some(&other)] {
        assert_eq!(
            service.status(&caller, authority).await,
            Err(LegalConsentError::RequiresOwnerSession)
        );
        assert_eq!(
            service.accept(&caller, authority, "1").await,
            Err(LegalConsentError::RequiresOwnerSession)
        );
    }
    assert!(store
        .acceptance(&caller.subject, "1")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn acceptance_is_recorded_once_for_the_current_version() {
    let store = Arc::new(MemoryLegalConsentStore::default());
    let service = service(Some("2026-11-30"), store.clone());
    let (caller, authority) = owner("owner");
    let before = service.status(&caller, Some(&authority)).await.unwrap();
    assert_eq!(before.terms.version(), "2026-11-30");
    assert_eq!(before.accepted_at, None);

    assert_eq!(
        service
            .accept(&caller, Some(&authority), "2026-11-01")
            .await,
        Err(LegalConsentError::VersionMismatch)
    );
    assert!(store
        .acceptance(&caller.subject, "2026-11-01")
        .await
        .unwrap()
        .is_none());

    let accepted = service
        .accept(&caller, Some(&authority), "2026-11-30")
        .await
        .unwrap();
    let at = accepted.accepted_at.clone().unwrap();
    assert!(crate::seconds_from_rfc3339(&at).is_some());
    let replayed = service
        .accept(&caller, Some(&authority), "2026-11-30")
        .await
        .unwrap();
    assert_eq!(replayed.accepted_at.as_deref(), Some(at.as_str()));
    assert_eq!(
        service
            .status(&caller, Some(&authority))
            .await
            .unwrap()
            .accepted_at,
        Some(at)
    );
}

#[tokio::test]
async fn a_new_version_requires_new_consent_and_keeps_the_earlier_record() {
    let store = Arc::new(MemoryLegalConsentStore::default());
    let (caller, authority) = owner("owner");
    service(Some("1"), store.clone())
        .accept(&caller, Some(&authority), "1")
        .await
        .unwrap();
    let next = service(Some("2"), store.clone());
    assert_eq!(
        next.status(&caller, Some(&authority))
            .await
            .unwrap()
            .accepted_at,
        None
    );
    next.accept(&caller, Some(&authority), "2").await.unwrap();
    assert!(store
        .acceptance(&caller.subject, "1")
        .await
        .unwrap()
        .is_some());
    let (other, other_authority) = owner("other-owner");
    assert_eq!(
        next.status(&other, Some(&other_authority))
            .await
            .unwrap()
            .accepted_at,
        None
    );
}

#[tokio::test]
async fn memory_store_retains_a_bounded_history() {
    let store = MemoryLegalConsentStore::default();
    let subject = SubjectId::new("owner");
    for index in 0..=MAX_RETAINED_ACCEPTANCES {
        store
            .accept(&subject, &index.to_string(), "2026-11-23T00:00:00Z")
            .await
            .unwrap();
    }
    assert!(store.acceptance(&subject, "0").await.unwrap().is_none());
    assert!(store
        .acceptance(&subject, &MAX_RETAINED_ACCEPTANCES.to_string())
        .await
        .unwrap()
        .is_some());
}

struct UnrecordedOwner;

#[async_trait]
impl LegalConsentStore for UnrecordedOwner {
    async fn acceptance(
        &self,
        _: &SubjectId,
        _: &str,
    ) -> Result<Option<ConsentAcceptance>, StoreError> {
        Ok(None)
    }

    async fn accept(
        &self,
        _: &SubjectId,
        _: &str,
        _: &str,
    ) -> Result<ConsentAcceptance, StoreError> {
        Err(StoreError::ReferenceUnavailable("no sign-in record".into()))
    }
}

#[tokio::test]
async fn an_owner_without_a_sign_in_record_is_asked_to_sign_in_again() {
    let service = LegalConsentService::new(Some(terms("1")), Arc::new(UnrecordedOwner));
    let (caller, authority) = owner("owner");
    assert_eq!(
        service.accept(&caller, Some(&authority), "1").await,
        Err(LegalConsentError::SignInRequired)
    );
}

struct FailingStore;

#[async_trait]
impl LegalConsentStore for FailingStore {
    async fn acceptance(
        &self,
        _: &SubjectId,
        _: &str,
    ) -> Result<Option<ConsentAcceptance>, StoreError> {
        Err(StoreError::Unavailable("offline".into()))
    }

    async fn accept(
        &self,
        _: &SubjectId,
        version: &str,
        _: &str,
    ) -> Result<ConsentAcceptance, StoreError> {
        Ok(ConsentAcceptance {
            version: format!("{version}-other"),
            accepted_at: "2026-11-23T00:00:00Z".into(),
        })
    }
}

#[tokio::test]
async fn store_failures_and_mismatched_records_are_refused() {
    let service = LegalConsentService::new(Some(terms("1")), Arc::new(FailingStore));
    let (caller, authority) = owner("owner");
    assert!(matches!(
        service.status(&caller, Some(&authority)).await,
        Err(LegalConsentError::Store(_))
    ));
    assert!(matches!(
        service.accept(&caller, Some(&authority), "1").await,
        Err(LegalConsentError::Store(_))
    ));
}
