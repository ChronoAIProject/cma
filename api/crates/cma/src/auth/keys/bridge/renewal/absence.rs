//! The detail endpoint conceals lost read access as 404. Only a successful
//! current-owner listing can establish absence after such a response.
use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
    keys: Vec<Key>,
}
#[derive(Deserialize)]
struct Key {
    id: String,
}

impl KeyIssuer {
    pub(super) async fn require_bridge_key_absent(
        &self,
        lease: &BridgeControlPlaneLease,
        provider_key_id: &str,
    ) -> Result<(), KeyIssuerError> {
        require_lease(lease)?;
        // At pinned NyxID85607255, this exact endpoint performs current
        // owner/admin resolution and returns all active keys without paging.
        let mut request = self.http.get(format!(
            "{}{}",
            self.base_url,
            super::super::super::KEYS_PATH
        ));
        if let ResourceOwner::Organization { subject } = lease.owner() {
            request = request.query(&[("org_id", subject.as_str())]);
        }
        let listing: Listing = self.bridge_json(request, lease).await?;
        if listing.keys.len() > super::super::super::PROVIDER_KEY_LIST_MAX_ROWS {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let mut ids = BTreeSet::new();
        for key in listing.keys {
            if !super::super::super::valid_provider_key_id(&key.id)
                || !ids.insert(key.id.clone())
                || key.id == provider_key_id
            {
                return Err(KeyIssuerError::UnexpectedAuthority);
            }
        }
        require_lease(lease)
    }
}
