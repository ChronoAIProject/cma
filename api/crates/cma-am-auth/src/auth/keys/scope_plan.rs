//! Exact configured-route authority comes from NyxID, never the service catalogue.
use super::*;
use layer2_core::resource_access::ResourceOwner;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Principal {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NodeGrant {
    NotRequired,
    Required { node_ids: Vec<String> },
}

#[derive(Debug, Deserialize)]
struct ServiceGrant {
    user_service_id: String,
    resource_owner: Principal,
    node_grant: NodeGrant,
}

#[derive(Debug, Deserialize)]
pub(super) struct ScopePlan {
    authority: String,
    contract_version: String,
    policy_version: String,
    authenticated_actor: Principal,
    intended_key_owner: Principal,
    services: Vec<ServiceGrant>,
    allowed_service_ids: Vec<String>,
    pub(super) allowed_node_ids: Vec<String>,
    evaluated_at: String,
    normalized_grant_digest: String,
    freshness: Value,
    completeness: Value,
    key_purpose: String,
    key_expires_at: Option<String>,
    #[serde(default)]
    durable_operations: Vec<Value>,
}

impl ScopePlan {
    pub(super) fn apply(&self, body: &mut Value) {
        body["allowed_node_ids"] = json!(self.allowed_node_ids);
        body["scope_plan_digest"] = self.normalized_grant_digest.clone().into();
    }

    fn validate(&self, selected: &[String], owner: &ResourceOwner) -> Option<()> {
        let expected_owner = match owner {
            ResourceOwner::Person { subject } => Principal {
                id: subject.to_string(),
                kind: "personal".into(),
            },
            ResourceOwner::Organization { subject } => Principal {
                id: subject.to_string(),
                kind: "organization".into(),
            },
        };
        let evaluated = bson::DateTime::parse_rfc3339_str(&self.evaluated_at)
            .ok()?
            .to_system_time();
        let now = SystemTime::now();
        if self.authority != "nyxid"
            || self.contract_version != "1"
            || self.policy_version != "api-key-scope-v1"
            || self.intended_key_owner != expected_owner
            || self.authenticated_actor.kind != "personal"
            || !valid_provider_key_id(&self.authenticated_actor.id)
            || (expected_owner.kind == "personal" && self.authenticated_actor != expected_owner)
            || evaluated > now + Duration::from_secs(60)
            || now.duration_since(evaluated).unwrap_or_default() > Duration::from_secs(300)
            || self.key_purpose != "general"
            || self.key_expires_at.is_some()
            || !self.durable_operations.is_empty()
            || self.freshness
                != json!({
                    "mode":"mutation_revalidated_snapshot", "precondition_field":"scope_plan_digest",
                    "post_creation_drift":"fail_closed"
                })
            || self.completeness
                != json!({
                    "list_complete":true,"no_duplicates":true,
                    "route_candidate_basis":"active_configured_routes","transient_node_state_excluded":true
                })
        {
            return None;
        }
        let selected = exact_ids(selected)?;
        if selected != exact_ids(&self.allowed_service_ids)?
            || self.services.len() != selected.len()
        {
            return None;
        }
        let mut services = BTreeSet::new();
        let mut nodes = BTreeSet::new();
        for grant in &self.services {
            if !services.insert(grant.user_service_id.as_str())
                || !selected.contains(grant.user_service_id.as_str())
                || !valid_provider_key_id(&grant.resource_owner.id)
                || !matches!(
                    grant.resource_owner.kind.as_str(),
                    "personal" | "organization"
                )
                || (grant.resource_owner.kind == "personal"
                    && grant.resource_owner != self.authenticated_actor)
                || (expected_owner.kind == "organization" && grant.resource_owner != expected_owner)
            {
                return None;
            }
            if let NodeGrant::Required { node_ids } = &grant.node_grant {
                if node_ids.is_empty() {
                    return None;
                }
                nodes.extend(exact_ids(node_ids)?);
            }
        }
        if nodes != exact_ids(&self.allowed_node_ids)?
            || self.normalized_grant_digest != self.digest()
        {
            return None;
        }
        Some(())
    }

    // The provider's v1 digest binds selected resources, owners and route nodes.
    // Sending it back makes mutation revalidate the same snapshot atomically.
    fn digest(&self) -> String {
        fn field(digest: &mut Sha256, value: &str) {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        fn count(digest: &mut Sha256, value: usize) {
            digest.update((value as u64).to_be_bytes());
        }
        let mut digest = Sha256::new();
        for value in [
            "nyxid-api-key-scope-plan-digest",
            &self.authority,
            &self.contract_version,
            &self.policy_version,
            &self.authenticated_actor.id,
            &self.authenticated_actor.kind,
            &self.intended_key_owner.id,
            &self.intended_key_owner.kind,
            "services",
        ] {
            field(&mut digest, value);
        }
        count(&mut digest, self.services.len());
        for service in &self.services {
            for value in [
                "service",
                &service.user_service_id,
                &service.resource_owner.id,
            ] {
                field(&mut digest, value);
            }
            match &service.node_grant {
                NodeGrant::NotRequired => field(&mut digest, "not_required"),
                NodeGrant::Required { node_ids } => {
                    field(&mut digest, "required");
                    count(&mut digest, node_ids.len());
                    for node in node_ids {
                        field(&mut digest, node);
                    }
                }
            }
        }
        for (label, ids) in [
            ("allowed_service_ids", &self.allowed_service_ids),
            ("allowed_node_ids", &self.allowed_node_ids),
        ] {
            field(&mut digest, label);
            count(&mut digest, ids.len());
            for id in ids {
                field(&mut digest, id);
            }
        }
        format!("sha256:{:x}", digest.finalize())
    }
}

fn exact_ids(ids: &[String]) -> Option<BTreeSet<&str>> {
    if ids.len() > 1000 || ids.iter().any(|id| !valid_provider_key_id(id)) {
        return None;
    }
    let values: BTreeSet<_> = ids.iter().map(String::as_str).collect();
    (values.len() == ids.len()).then_some(values)
}

impl KeyIssuer {
    pub(super) async fn scope_plan(
        &self,
        token: &str,
        selected: &[String],
        owner: &ResourceOwner,
    ) -> Result<ScopePlan, KeyIssuerError> {
        let mut body = json!({"selected_service_ids":selected});
        if let ResourceOwner::Organization { subject } = owner {
            body["target_org_id"] = subject.as_str().into();
        }
        let response = self
            .http
            .post(format!("{}{KEYS_PATH}/scope-plan", self.base_url))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|_| {
                KeyIssuerError::Unavailable("service route authority unavailable".into())
            })?;
        let plan: ScopePlan = super::super::provider_http::read_json(response)
            .await
            .map_err(|error| {
                use super::super::provider_http::ProviderReadError;
                match error {
                    ProviderReadError::Refused(status) => KeyIssuerError::Refused(status),
                    ProviderReadError::Unavailable => {
                        KeyIssuerError::Unavailable("service route authority unavailable".into())
                    }
                    ProviderReadError::Invalid => KeyIssuerError::UnexpectedAuthority,
                }
            })?;
        plan.validate(selected, owner)
            .ok_or(KeyIssuerError::UnexpectedAuthority)?;
        Ok(plan)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod tests;
