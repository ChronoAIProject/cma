//! Validate the exact issued grant and persist its authority fingerprint.
use super::*;

fn normalized_identifiers(values: &[String]) -> Option<Vec<&str>> {
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim();
        if value.is_empty()
            || value
                .bytes()
                .filter(u8::is_ascii_hexdigit)
                .all(|byte| byte == b'0')
            || normalized.contains(&value)
        {
            return None;
        }
        normalized.push(value);
    }
    normalized.sort_unstable();
    Some(normalized)
}

fn provider_grant_fingerprint(response: &MintedKeyResponse) -> Option<String> {
    let services = normalized_identifiers(&response.allowed_service_ids)?;
    let nodes = normalized_identifiers(&response.allowed_node_ids)?;
    let mut digest = Sha256::new();
    digest.update(b"cma-nyxid-resource-key-grant-v1\0");
    let scopes = response.scopes.trim();
    digest.update((scopes.len() as u64).to_be_bytes());
    digest.update(scopes.as_bytes());
    for collection in [services, nodes] {
        digest.update((collection.len() as u64).to_be_bytes());
        for value in collection {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }
    digest.update([u8::from(response.allow_all_services)]);
    digest.update([u8::from(response.allow_all_nodes)]);
    Some(format!("{:x}", digest.finalize()))
}

pub(super) fn minted_key_from_response(
    response: MintedKeyResponse,
    expected_service_ids: &[String],
    expected_node_ids: &[String],
    expires_at: SystemTime,
) -> Option<MintedKey> {
    let expected = normalized_identifiers(expected_service_ids)?;
    let returned = normalized_identifiers(&response.allowed_service_ids)?;
    if !valid_provider_key_id(&response.id)
        || response.full_key.trim().is_empty()
        || response.scopes != KEY_SCOPES
        || response.state_version <= 0
        || returned != expected
        || normalized_identifiers(&response.allowed_node_ids)?
            != normalized_identifiers(expected_node_ids)?
        || response.allow_all_services
        || response.allow_all_nodes
        || response.allow_auto_connected_services
    {
        return None;
    }
    let grant_fingerprint = provider_grant_fingerprint(&response)?;
    Some(MintedKey {
        id: response.id,
        value: SecretValue::new(response.full_key),
        expires_at,
        state_version: response.state_version,
        grant_fingerprint,
        allowed_service_ids: response.allowed_service_ids,
        allowed_node_ids: response.allowed_node_ids,
    })
}

impl MintedKey {
    pub fn matches_grant(&self, services: &[String], nodes: &[String]) -> bool {
        normalized_identifiers(services).is_some_and(|observed| {
            Some(observed) == normalized_identifiers(&self.allowed_service_ids)
        }) && normalized_identifiers(nodes).is_some_and(|observed| {
            Some(observed) == normalized_identifiers(&self.allowed_node_ids)
        })
    }
}
