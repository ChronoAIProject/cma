use super::super::{KeyIssuerError, MintedKey, MintedKeyResponse};

#[cfg(test)]
#[path = "contract_tests.rs"]
mod tests;
use layer2_core::{resource_access::ResourceOwner, ServiceId, SubjectId};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ServiceList {
    pub services: Vec<Service>,
}
#[derive(Deserialize)]
pub(super) struct Service {
    id: String,
    slug: Option<String>,
    is_active: bool,
    credential_source: Source,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Source {
    Personal,
    Org {
        org_id: String,
        role: String,
        allowed: bool,
    },
}
impl Source {
    fn matches(&self, owner: &ResourceOwner, actor: &SubjectId) -> bool {
        match (self, owner) {
            (Self::Personal, ResourceOwner::Person { subject }) => subject == actor,
            (
                Self::Org {
                    org_id,
                    role,
                    allowed,
                },
                ResourceOwner::Organization { subject },
            ) => org_id == subject.as_str() && role == "admin" && *allowed,
            _ => false,
        }
    }
}
pub(super) fn select_services(
    owner: &ResourceOwner,
    actor: &SubjectId,
    services: &[Service],
    wanted: &[ServiceId],
) -> Result<Vec<String>, KeyIssuerError> {
    if services.len() > 1000 {
        return Err(KeyIssuerError::UnexpectedAuthority);
    }
    let mut selected = std::collections::BTreeSet::new();
    for wanted in wanted {
        let mut candidates = services.iter().filter(|row| {
            row.is_active
                && (row.id == wanted.as_str() || row.slug.as_deref() == Some(wanted.as_str()))
                && row.credential_source.matches(owner, actor)
        });
        let found = candidates
            .next()
            .ok_or_else(|| KeyIssuerError::ServiceNotGranted(wanted.clone()))?;
        if candidates.next().is_some() || !super::super::valid_provider_key_id(&found.id) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        selected.insert(found.id.clone());
    }
    Ok(selected.into_iter().collect())
}
#[derive(Deserialize)]
struct Principal {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}
impl Principal {
    fn matches(&self, owner: &ResourceOwner) -> bool {
        match owner {
            ResourceOwner::Person { subject } => {
                self.kind == "personal" && self.id == subject.as_str()
            }
            ResourceOwner::Organization { subject } => {
                self.kind == "organization" && self.id == subject.as_str()
            }
        }
    }
}
#[derive(Deserialize)]
struct Completeness {
    list_complete: bool,
    no_duplicates: bool,
    route_candidate_basis: String,
    transient_node_state_excluded: bool,
}
#[derive(Deserialize)]
struct Freshness {
    mode: String,
    precondition_field: String,
    post_creation_drift: String,
}
#[derive(Deserialize)]
struct ServiceGrant {
    user_service_id: String,
    resource_owner: Principal,
    node_grant: NodeGrant,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NodeGrant {
    NotRequired,
    Required { node_ids: Vec<String> },
}
#[derive(Deserialize)]
pub(super) struct ScopePlan {
    authority: String,
    contract_version: String,
    policy_version: String,
    authenticated_actor: Principal,
    intended_key_owner: Principal,
    services: Vec<ServiceGrant>,
    pub allowed_service_ids: Vec<String>,
    pub allowed_node_ids: Vec<String>,
    pub normalized_grant_digest: String,
    completeness: Completeness,
    freshness: Freshness,
    key_purpose: String,
    #[serde(default)]
    durable_operations: Vec<serde_json::Value>,
}
impl ScopePlan {
    pub fn require(
        &self,
        owner: &ResourceOwner,
        actor: &SubjectId,
        selected: &[String],
    ) -> Result<(), KeyIssuerError> {
        let mut grants = std::collections::BTreeSet::new();
        let mut nodes = std::collections::BTreeSet::new();
        for grant in &self.services {
            if !grant.resource_owner.matches(owner) || !grants.insert(grant.user_service_id.clone())
            {
                return Err(KeyIssuerError::UnexpectedAuthority);
            }
            if let NodeGrant::Required { node_ids } = &grant.node_grant {
                if node_ids.is_empty() || super::super::normalized_identifiers(node_ids).is_none() {
                    return Err(KeyIssuerError::UnexpectedAuthority);
                }
                nodes.extend(node_ids.iter().cloned());
            }
        }
        if self.authority != "nyxid"
            || self.contract_version != "1"
            || self.policy_version != "api-key-scope-v1"
            || !self.authenticated_actor.matches(&ResourceOwner::Person {
                subject: actor.clone(),
            })
            || !self.intended_key_owner.matches(owner)
            || self.key_purpose != "general"
            || !self.durable_operations.is_empty()
            || super::super::normalized_identifiers(&self.allowed_service_ids)
                != super::super::normalized_identifiers(selected)
            || super::super::normalized_identifiers(&self.allowed_node_ids).is_none()
            || grants.into_iter().collect::<Vec<_>>() != selected
            || nodes.into_iter().collect::<Vec<_>>() != self.allowed_node_ids
            || self.normalized_grant_digest.len() != 64
            || !self
                .normalized_grant_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !self.completeness.list_complete
            || !self.completeness.no_duplicates
            || !self.completeness.transient_node_state_excluded
            || self.completeness.route_candidate_basis != "active_configured_routes"
            || self.freshness.mode != "mutation_revalidated_snapshot"
            || self.freshness.precondition_field != "scope_plan_digest"
            || self.freshness.post_creation_drift != "fail_closed"
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(())
    }
}

pub(super) fn material(
    response: MintedKeyResponse,
    plan: &ScopePlan,
    expires_at: std::time::SystemTime,
) -> Result<MintedKey, KeyIssuerError> {
    if !super::super::valid_provider_key_id(&response.id)
        || response.full_key.trim().is_empty()
        || response.scopes != super::super::KEY_SCOPES
        || response.state_version <= 0
        || super::super::normalized_identifiers(&response.allowed_service_ids)
            != super::super::normalized_identifiers(&plan.allowed_service_ids)
        || super::super::normalized_identifiers(&response.allowed_node_ids)
            != super::super::normalized_identifiers(&plan.allowed_node_ids)
        || response.allow_all_services
        || response.allow_all_nodes
    {
        return Err(KeyIssuerError::UnexpectedAuthority);
    }
    Ok(MintedKey {
        grant_fingerprint: super::super::provider_grant_fingerprint(&response)
            .ok_or(KeyIssuerError::UnexpectedAuthority)?,
        id: response.id,
        value: crate::auth::persistence::SecretValue::new(response.full_key),
        expires_at,
        state_version: response.state_version,
    })
}
#[derive(Deserialize)]
pub(super) struct KeyMetadata {
    id: String,
    name: String,
    is_active: bool,
    expires_at: String,
    state_version: i64,
    scopes: String,
    allowed_service_ids: Vec<String>,
    allowed_node_ids: Vec<String>,
    allow_all_services: bool,
    allow_all_nodes: bool,
    credential_source: Source,
    purpose: String,
    scheduled_write_enabled: bool,
}
impl KeyMetadata {
    pub fn require(
        &self,
        owner: &ResourceOwner,
        actor: &SubjectId,
        name: &str,
        material: &MintedKey,
        plan: &ScopePlan,
    ) -> Result<(), KeyIssuerError> {
        let expires_at = mongodb::bson::DateTime::parse_rfc3339_str(&self.expires_at)
            .ok()
            .map(|value| value.to_system_time());
        if self.id != material.id
            || self.name != name
            || !self.is_active
            || self.state_version != material.state_version
            || expires_at != Some(material.expires_at)
            || self.scopes != super::super::KEY_SCOPES
            || !self.credential_source.matches(owner, actor)
            || self.purpose != "general"
            || self.scheduled_write_enabled
            || super::super::normalized_identifiers(&self.allowed_service_ids)
                != super::super::normalized_identifiers(&plan.allowed_service_ids)
            || super::super::normalized_identifiers(&self.allowed_node_ids)
                != super::super::normalized_identifiers(&plan.allowed_node_ids)
            || self.allow_all_services
            || self.allow_all_nodes
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(())
    }
}
