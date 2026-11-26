//! Owner acceptance of the operator-published terms and privacy notice.
use std::sync::Arc;

use crate::{config::Config, store::MongoStores};
use layer2_core::legal_consent::LegalConsentService;

pub(super) fn service(config: &Config, stores: &MongoStores) -> Arc<LegalConsentService> {
    Arc::new(LegalConsentService::new(
        config.legal_terms.clone(),
        stores.legal_consents.clone(),
    ))
}
