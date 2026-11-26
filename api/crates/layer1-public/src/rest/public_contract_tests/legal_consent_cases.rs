use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::{
    account_deletion::OwnerSessionAuthority,
    legal_consent::{
        ConsentAcceptance, LegalConsentService, LegalConsentStore, LegalTerms,
        MemoryLegalConsentStore,
    },
    Authorization, Capability, StoreError, SubjectId,
};

use super::Scenario;

struct UnavailableStore;

#[async_trait]
impl LegalConsentStore for UnavailableStore {
    async fn acceptance(
        &self,
        _: &SubjectId,
        _: &str,
    ) -> Result<Option<ConsentAcceptance>, StoreError> {
        Err(StoreError::Unavailable(
            "contract legal consent store".into(),
        ))
    }

    async fn accept(
        &self,
        _: &SubjectId,
        _: &str,
        _: &str,
    ) -> Result<ConsentAcceptance, StoreError> {
        Err(StoreError::Unavailable(
            "contract legal consent store".into(),
        ))
    }
}

/// An owner whose browser sign-in left no directory record.
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
        Err(StoreError::ReferenceUnavailable(
            "contract owner has no sign-in record".into(),
        ))
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("legal_consent_")?;
    let store: Arc<dyn LegalConsentStore> = match mode {
        "store_error" => Arc::new(UnavailableStore),
        "unrecorded" => Arc::new(UnrecordedOwner),
        "owner" | "unconfigured" | "anonymous" | "nonhuman" => {
            Arc::new(MemoryLegalConsentStore::default())
        }
        _ => panic!("unknown legal consent contract scenario {mode}"),
    };
    let terms = (mode != "unconfigured").then(|| {
        LegalTerms::new(
            "2026-11-30",
            "https://legal.example.test/terms",
            "https://legal.example.test/privacy",
        )
        .unwrap()
    });
    let router = super::super::legal_consent::legal_consent_router(Arc::new(
        LegalConsentService::new(terms, store),
    ));
    let router = if mode == "nonhuman" || mode == "anonymous" {
        router
    } else {
        router.layer(axum::Extension(OwnerSessionAuthority::verified_personal(
            SubjectId::new("owner"),
        )))
    };
    Some(Scenario {
        router,
        subject: (mode != "anonymous").then_some("owner"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        symbols: Default::default(),
    })
}
